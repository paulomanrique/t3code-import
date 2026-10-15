//! Closing T3 Code before an import, and starting its background service again
//! afterwards.
//!
//! A supervised server can't just be killed: launchd (`KeepAlive`) and systemd
//! (`Restart=`) start it again within a second. So the supervisor is asked to
//! stop the job, and to start it again once the import is done. The desktop app
//! and servers started by hand get SIGTERM and stay closed.
//!
//! Nothing is stopped on a hunch. A supervisor counts only when the program it
//! runs is a T3 launcher, a job is stopped only when t3-import knows how to start
//! it again, and a process is signalled only after it is confirmed, right before
//! the signal, to still be the same T3 process (same PID and start time).

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use sysinfo::{Pid, Signal, System};

use super::{T3Home, guard};

/// How long one launchctl/systemctl call may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
/// How long T3 gets to let go of its database once asked to close.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// A launchd agent (macOS). `plist` is what `bootstrap` needs to load it again.
    Launchd {
        domain: String,
        label: String,
        plist: Option<PathBuf>,
    },
    /// A systemd user unit (Linux), such as the one `t3 service install` sets up.
    SystemdUser { unit: String },
    /// A system-wide unit. Stopping it needs root, so t3-import leaves it alone.
    SystemdSystem { unit: String },
    /// The T3 Code desktop app.
    App,
    /// A server started by hand, such as `t3 serve` in a terminal.
    Process,
    /// A PID recorded in server-runtime.json that doesn't look like T3. The file
    /// may be stale, so t3-import won't touch whatever has that PID now.
    Unverified,
}

/// One thing to close: a supervised job, the app, or loose server processes.
#[derive(Debug, Clone)]
pub struct RunningT3 {
    pub owner: Owner,
    /// PID and start time of each process, so a reused PID is never signalled.
    pub processes: Vec<(u64, u64)>,
}

impl RunningT3 {
    fn pid_list(&self) -> String {
        self.processes
            .iter()
            .map(|(pid, _)| pid.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// What is running, in a few words.
    pub fn name(&self) -> String {
        match &self.owner {
            Owner::Launchd { label, .. } => format!("T3 background service ({label}, launchd)"),
            Owner::SystemdUser { unit } => format!("T3 background service ({unit}, systemd)"),
            Owner::SystemdSystem { unit } => format!("T3 system service ({unit})"),
            Owner::App => "T3 Code app".into(),
            Owner::Process => format!("T3 server started by hand (PID {})", self.pid_list()),
            Owner::Unverified => format!("PID {} from T3's server-runtime.json", self.pid_list()),
        }
    }

    /// What closing it involves.
    pub fn plan(&self) -> String {
        match &self.owner {
            Owner::Launchd { plist: Some(_), .. } | Owner::SystemdUser { .. } => {
                "stopped, then started again after the import".into()
            }
            Owner::Launchd { domain, label, plist: None } => format!(
                "t3-import can't start it again, so it won't stop it. Stop it with `launchctl bootout {domain}/{label}` and start it yourself afterwards"
            ),
            Owner::SystemdSystem { unit } => format!("needs root: run `sudo systemctl stop {unit}` first"),
            Owner::App => "closed; open it again after the import".into(),
            Owner::Process => "closed, and not started again".into(),
            Owner::Unverified => {
                "it doesn't look like T3, so t3-import won't close it. If T3 isn't running, that file is stale".into()
            }
        }
    }

    pub fn can_close(&self) -> bool {
        matches!(
            self.owner,
            Owner::Launchd { plist: Some(_), .. }
                | Owner::SystemdUser { .. }
                | Owner::App
                | Owner::Process
        )
    }
}

/// Supervised jobs t3-import stopped and owes a start.
#[derive(Debug, Clone, Default)]
pub struct Restart {
    jobs: Vec<Owner>,
}

impl Restart {
    pub fn names(&self) -> Vec<String> {
        self.jobs
            .iter()
            .map(|owner| {
                RunningT3 {
                    owner: owner.clone(),
                    processes: vec![],
                }
                .name()
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn extend(&mut self, other: Restart) {
        self.jobs.extend(other.jobs);
    }

    /// Starts every stopped job again, one line per job saying how it went. Jobs
    /// that fail to start stay owed, so the caller can try again.
    pub fn run(&mut self) -> Vec<Result<String, String>> {
        self.run_with(start)
    }

    fn run_with(
        &mut self,
        start_job: impl Fn(&Owner) -> Result<String, String>,
    ) -> Vec<Result<String, String>> {
        let mut failed = Vec::new();
        let results = self
            .jobs
            .clone()
            .into_iter()
            .map(|job| {
                let result = start_job(&job);
                if result.is_err() {
                    failed.push(job);
                }
                result
            })
            .collect();
        self.jobs = failed;
        results
    }
}

fn start(job: &Owner) -> Result<String, String> {
    match job {
        Owner::Launchd {
            domain,
            label,
            plist: Some(plist),
        } => run(Command::new("launchctl")
            .arg("bootstrap")
            .arg(domain)
            .arg(plist))
        .map(|()| format!("Started the T3 background service {label} again."))
        .map_err(|e| {
            format!(
                "Couldn't start {label} again: {e:#}. Run: launchctl bootstrap {domain} {}",
                plist.display()
            )
        }),
        Owner::SystemdUser { unit } => {
            run(Command::new("systemctl").args(["--user", "start", unit]))
                .map(|()| format!("Started the T3 background service {unit} again."))
                .map_err(|e| {
                    format!(
                        "Couldn't start {unit} again: {e:#}. Run: systemctl --user start {unit}"
                    )
                })
        }
        other => Err(format!("t3-import doesn't start {other:?}")),
    }
}

/// What is keeping T3's database busy, grouped by what has to be closed.
pub fn find(home: &T3Home) -> anyhow::Result<Vec<RunningT3>> {
    let system = System::new_all();
    let blocking = guard::running_pids(home, &system)?;
    let verified = guard::t3_processes(home, &system);
    let launchd = if cfg!(target_os = "macos") {
        launchd_jobs()
    } else {
        HashMap::new()
    };

    let mut grouped: BTreeMap<String, RunningT3> = BTreeMap::new();
    for &pid in blocking.keys() {
        let (owner, started) = match verified.get(&pid) {
            Some(&started) => (owner_of(pid, &system, &launchd), started),
            None => (
                Owner::Unverified,
                system
                    .process(Pid::from_u32(pid as u32))
                    .map_or(0, |p| p.start_time()),
            ),
        };
        let key = format!("{owner:?}");
        grouped
            .entry(key)
            .or_insert_with(|| RunningT3 {
                owner,
                processes: Vec::new(),
            })
            .processes
            .push((pid, started));
    }
    Ok(grouped.into_values().collect())
}

/// Closes everything in `running` and waits until T3 has let go of its database.
/// If T3 doesn't go away, whatever was stopped is started again before the error
/// is returned.
pub fn close(home: &T3Home, running: &[RunningT3]) -> anyhow::Result<Restart> {
    let mut restart = Restart::default();
    match close_into(home, running, &mut restart) {
        Ok(()) => Ok(restart),
        Err(e) => {
            let restarted: Vec<String> = restart
                .run()
                .into_iter()
                .map(|r| r.unwrap_or_else(|e| e))
                .collect();
            if restarted.is_empty() {
                Err(e)
            } else {
                Err(e.context(restarted.join(" ")))
            }
        }
    }
}

/// Records stopped services even when closing fails. The caller owns recovery.
pub fn close_into(
    home: &T3Home,
    running: &[RunningT3],
    restart: &mut Restart,
) -> anyhow::Result<()> {
    if let Some(blocked) = running.iter().find(|r| !r.can_close()) {
        bail!("{}: {}", blocked.name(), blocked.plan());
    }
    for item in running {
        match &item.owner {
            Owner::Launchd {
                domain,
                label,
                plist: Some(_),
            } => {
                run(Command::new("launchctl")
                    .arg("bootout")
                    .arg(format!("{domain}/{label}")))
                .with_context(|| format!("stop {label}"))?;
                restart.jobs.push(item.owner.clone());
            }
            Owner::SystemdUser { unit } => {
                run(Command::new("systemctl").args(["--user", "stop", unit]))
                    .with_context(|| format!("stop {unit}"))?;
                restart.jobs.push(item.owner.clone());
            }
            Owner::App | Owner::Process => {
                // Re-identify right before signalling: the PID must still be a
                // T3 process for this home, started at the same moment.
                let system = System::new_all();
                let now = guard::t3_processes(home, &system);
                for &(pid, started) in &item.processes {
                    if now.get(&pid) != Some(&started) {
                        continue;
                    }
                    if let Some(process) = system.process(Pid::from_u32(pid as u32)) {
                        // Windows has no SIGTERM; `kill` is the only option there.
                        if process.kill_with(Signal::Term).is_none() {
                            process.kill();
                        }
                    }
                }
            }
            _ => unreachable!("checked by can_close"),
        }
    }
    wait_until_closed(home, CLOSE_TIMEOUT)
}

fn wait_until_closed(home: &T3Home, timeout: Duration) -> anyhow::Result<()> {
    let start = Instant::now();
    loop {
        let left = guard::running_pids(home, &System::new_all())?;
        if left.is_empty() {
            return Ok(());
        }
        if start.elapsed() > timeout {
            let left = left.into_values().collect::<Vec<_>>().join("; ");
            bail!("T3 is still running after {}s: {left}", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn run(cmd: &mut Command) -> anyhow::Result<()> {
    let out = output_timeout(cmd, COMMAND_TIMEOUT)?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{} ({})", err.trim(), out.status);
    }
    Ok(())
}

/// `Command::output` with a time limit: a hung child is killed, not waited on.
pub(crate) fn output_timeout(cmd: &mut Command, timeout: Duration) -> anyhow::Result<Output> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run {program}"))?;
    // Drain both pipes while waiting, so a chatty child can't block on a full pipe.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{program} didn't finish within {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn uid() -> anyhow::Result<String> {
    let out = output_timeout(Command::new("id").arg("-u"), COMMAND_TIMEOUT)?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    anyhow::ensure!(
        !uid.is_empty() && uid.chars().all(|c| c.is_ascii_digit()),
        "can't tell the user id"
    );
    Ok(uid)
}

fn owner_of(pid: u64, system: &System, launchd: &HashMap<u64, String>) -> Owner {
    let mut chain = Vec::new();
    let mut cur = Some(Pid::from_u32(pid as u32));
    while let Some(p) = cur {
        if chain.len() > 32 || p.as_u32() <= 1 {
            break;
        }
        chain.push(u64::from(p.as_u32()));
        cur = system.process(p).and_then(|proc| proc.parent());
    }
    let argv_of = |pid: u64| -> Vec<String> {
        system
            .process(Pid::from_u32(pid as u32))
            .map(|p| {
                p.cmd()
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    match launchd_owner(&chain, launchd, argv_of) {
        Some(Owner::Launchd { label, .. }) => {
            let (domain, plist) = launchd_locate(&label);
            return Owner::Launchd {
                domain,
                label,
                plist,
            };
        }
        Some(owner) => return owner,
        None => {}
    }
    #[cfg(target_os = "linux")]
    if let Some(owner) = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .ok()
        .and_then(|c| systemd_owner(&c, systemd_exec_start))
    {
        return owner;
    }
    let exe = system
        .process(Pid::from_u32(pid as u32))
        .and_then(|p| p.exe())
        .map(|p| p.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if exe.contains(".app/contents/")
        || exe.contains("t3code")
        || exe.contains("t3-code")
        || exe.contains("t3 code")
    {
        Owner::App
    } else {
        Owner::Process
    }
}

/// True when `argv` runs a T3 launcher: the `t3` CLI (native or through node),
/// or T3's server entry point.
fn is_t3_launcher<S: AsRef<str>>(argv: &[S]) -> bool {
    argv.iter().any(|arg| {
        let arg = arg.as_ref().to_ascii_lowercase().replace('\\', "/");
        let name = arg.rsplit('/').next().unwrap_or("");
        matches!(name, "t3" | "t3.exe" | "t3.cmd") || arg.ends_with("/apps/server/dist/bin.mjs")
    })
}

/// The nearest launchd job among `chain` (the process and its ancestors), if it
/// is T3's. A server started from a terminal has the terminal's job above it, and
/// that must never be booted out, so the job's own program has to be a T3
/// launcher; an app opened from the Dock or Finder counts when it is T3's app.
fn launchd_owner(
    chain: &[u64],
    jobs: &HashMap<u64, String>,
    argv_of: impl Fn(u64) -> Vec<String>,
) -> Option<Owner> {
    let (job_pid, label) = chain
        .iter()
        .find_map(|pid| jobs.get(pid).map(|label| (*pid, label)))?;
    if label.starts_with("application.") {
        let lower = label.to_ascii_lowercase();
        return (lower.contains("t3code") || lower.contains("t3tools")).then_some(Owner::App);
    }
    is_t3_launcher(&argv_of(job_pid)).then(|| Owner::Launchd {
        domain: String::new(),
        label: label.clone(),
        plist: None,
    })
}

/// PID → label for the user's running launchd jobs.
fn launchd_jobs() -> HashMap<u64, String> {
    output_timeout(Command::new("launchctl").arg("list"), COMMAND_TIMEOUT)
        .map(|out| parse_launchctl_list(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

fn parse_launchctl_list(text: &str) -> HashMap<u64, String> {
    text.lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let pid = cols.next()?.trim().parse().ok()?;
            let _status = cols.next()?;
            Some((pid, cols.next()?.trim().to_string()))
        })
        .collect()
}

/// The domain the job lives in and the plist it was loaded from. `launchctl list`
/// doesn't say which domain a job is in, so ask the GUI domain, then the user one.
fn launchd_locate(label: &str) -> (String, Option<PathBuf>) {
    let Ok(uid) = uid() else {
        return (String::new(), None);
    };
    for domain in [format!("gui/{uid}"), format!("user/{uid}")] {
        let Ok(out) = output_timeout(
            Command::new("launchctl")
                .arg("print")
                .arg(format!("{domain}/{label}")),
            COMMAND_TIMEOUT,
        ) else {
            continue;
        };
        if out.status.success() {
            return (
                domain,
                parse_launchctl_print_path(&String::from_utf8_lossy(&out.stdout)),
            );
        }
    }
    (format!("gui/{uid}"), None)
}

fn parse_launchctl_print_path(text: &str) -> Option<PathBuf> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("path = "))
        .map(|p| PathBuf::from(p.trim()))
        .filter(|p| p.extension().is_some_and(|e| e == "plist") && p.is_file())
}

#[cfg(target_os = "linux")]
fn systemd_exec_start(unit: &str, user: bool) -> String {
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    cmd.args(["show", "--property=ExecStart", "--value", unit]);
    output_timeout(&mut cmd, COMMAND_TIMEOUT)
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

/// The systemd unit a process runs under, from `/proc/<pid>/cgroup`, if it is
/// T3's. As with launchd, the unit's own command has to be a T3 launcher: a
/// terminal emulator can itself be a user service, and stopping it would close
/// the user's terminal.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn systemd_owner(cgroup: &str, exec_start: impl Fn(&str, bool) -> String) -> Option<Owner> {
    let path = cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::").or_else(|| l.split(':').nth(2)))?;
    let unit = path
        .split('/')
        .rev()
        .find(|seg| seg.ends_with(".service"))?;
    if unit.starts_with("user@") {
        return None;
    }
    let user = path.contains("/user@");
    let exec = exec_start(unit, user);
    if !is_t3_launcher(&exec.split_whitespace().collect::<Vec<_>>()) {
        return None;
    }
    let unit = unit.to_string();
    Some(if user {
        Owner::SystemdUser { unit }
    } else {
        Owner::SystemdSystem { unit }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchctl_list_maps_pids_to_labels() {
        let jobs = parse_launchctl_list(
            "PID\tStatus\tLabel\n-\t0\tcom.t3code.update\n823\t0\tcom.t3code.daemon\n",
        );
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[&823], "com.t3code.daemon");
    }

    #[test]
    fn launchd_owner_needs_a_t3_program() {
        let jobs: HashMap<u64, String> = [
            (823, "com.t3code.daemon".to_string()),
            (500, "application.com.googlecode.iterm2.1234".to_string()),
            (600, "application.com.t3tools.t3code.99".to_string()),
            (900, "com.example.project3.terminal".to_string()),
        ]
        .into();
        let argv = |pid: u64| -> Vec<String> {
            match pid {
                823 => vec![
                    "/opt/homebrew/bin/node".into(),
                    "/opt/homebrew/bin/t3".into(),
                    "serve".into(),
                ],
                900 => vec!["/Applications/Term.app/Contents/MacOS/term".into()],
                _ => vec![],
            }
        };
        // Server under the daemon's node wrapper.
        assert!(matches!(
            launchd_owner(&[2696, 823], &jobs, argv),
            Some(Owner::Launchd { label, .. }) if label == "com.t3code.daemon"
        ));
        // `t3 serve` typed into iTerm: never boot out the terminal.
        assert_eq!(launchd_owner(&[4000, 3999, 500], &jobs, argv), None);
        // A terminal job whose label happens to contain "t3".
        assert_eq!(launchd_owner(&[4100, 900], &jobs, argv), None);
        // The app opened from the Dock.
        assert_eq!(launchd_owner(&[700, 600], &jobs, argv), Some(Owner::App));
    }

    #[test]
    fn launchctl_print_path() {
        let dir = tempfile::tempdir().unwrap();
        let plist = dir.path().join("com.t3code.daemon.plist");
        std::fs::write(&plist, "").unwrap();
        let text = format!(
            "gui/501/com.t3code.daemon = {{\n\tactive count = 1\n\tpath = {}\n\ttype = LaunchAgent\n",
            plist.display()
        );
        assert_eq!(parse_launchctl_print_path(&text), Some(plist));
        assert_eq!(
            parse_launchctl_print_path("path = (submitted by launchctl)"),
            None
        );
        assert_eq!(
            parse_launchctl_print_path("path = /nowhere/gone.plist"),
            None
        );
    }

    #[test]
    fn systemd_units_need_a_t3_command() {
        let exec = |unit: &str, _user: bool| {
            match unit {
            "t3code.service" | "t3-code.service" => "{ path=/usr/bin/t3 ; argv[]=/usr/bin/t3 serve ; }".to_string(),
            _ => "{ path=/usr/libexec/gnome-terminal-server ; argv[]=/usr/libexec/gnome-terminal-server ; }".to_string(),
        }
        };
        assert_eq!(
            systemd_owner(
                "0::/user.slice/user-1000.slice/user@1000.service/app.slice/t3code.service\n",
                exec
            ),
            Some(Owner::SystemdUser {
                unit: "t3code.service".into()
            })
        );
        assert_eq!(
            systemd_owner("0::/system.slice/t3-code.service\n", exec),
            Some(Owner::SystemdSystem {
                unit: "t3-code.service".into()
            })
        );
        // A server typed into a terminal that runs as a user service.
        assert_eq!(
            systemd_owner(
                "0::/user.slice/user-1000.slice/user@1000.service/app.slice/gnome-terminal-server.service\n",
                exec
            ),
            None
        );
        assert_eq!(
            systemd_owner(
                "0::/user.slice/user-1000.slice/user@1000.service/app.slice/project3-terminal.service\n",
                exec
            ),
            None
        );
        assert_eq!(
            systemd_owner("0::/user.slice/user-1000.slice/session-2.scope\n", exec),
            None
        );
    }

    #[test]
    fn t3_launchers() {
        assert!(is_t3_launcher(&["node", "/opt/homebrew/bin/t3", "serve"]));
        assert!(is_t3_launcher(&[
            "/x/T3 Code.app/Contents/MacOS/T3 Code",
            "/x/app.asar/apps/server/dist/bin.mjs"
        ]));
        assert!(is_t3_launcher(&[
            "C:\\Users\\me\\AppData\\Roaming\\npm\\t3.cmd",
            "serve"
        ]));
        assert!(!is_t3_launcher(&["/usr/bin/zsh", "-l"]));
        assert!(!is_t3_launcher(&["/opt/t3x/bin/server"]));
    }

    #[test]
    fn restart_keeps_jobs_that_fail_to_start() {
        let mut restart = Restart {
            jobs: vec![Owner::App],
        };
        let results = restart.run();
        assert!(results[0].is_err());
        assert!(!restart.is_empty());
    }

    #[test]
    fn restart_panic_keeps_pending_jobs() {
        let mut restart = Restart {
            jobs: vec![Owner::App],
        };
        let before = restart.names();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restart.run_with(|_| panic!("restart panic"));
        }));
        assert!(result.is_err());
        assert_eq!(restart.names(), before);
        assert!(!restart.is_empty());
    }

    #[test]
    fn output_timeout_kills_a_hung_child() {
        if cfg!(windows) {
            return;
        }
        let start = Instant::now();
        let err = output_timeout(Command::new("sleep").arg("30"), Duration::from_millis(300))
            .unwrap_err();
        assert!(err.to_string().contains("didn't finish"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
