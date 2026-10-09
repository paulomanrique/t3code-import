use super::{T3Home, payload};
use anyhow::{Context, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use sysinfo::{Pid, System};

const GITHUB_REPO: &str = "paulomanrique/t3code-import";
pub(super) const TABLES: [&str; 8] = [
    "orchestration_events",
    "orchestration_v2_projection_threads",
    "orchestration_v2_projection_messages",
    "orchestration_v2_projection_turn_items",
    "orchestration_v2_projection_provider_threads",
    "orchestration_v2_turn_item_positions",
    "provider_session_runtime",
    "orchestration_v2_projection_metadata",
];
pub(super) fn readonly(home: &T3Home) -> anyhow::Result<Connection> {
    Connection::open_with_flags(home.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("open T3 database read-only")
}

#[derive(Debug, Clone)]
pub struct GuardFinding {
    /// Table, column or payload field that differs.
    pub location: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Default)]
pub struct GuardReport {
    /// T3 server/app/service processes for this home that are still alive.
    pub t3_running: Vec<String>,
    pub findings: Vec<GuardFinding>,
    /// Rows sampled per table for the payload-shape check.
    pub sampled: Vec<(String, usize)>,
    pub warnings: Vec<String>,
    /// Prefilled GitHub "new issue" URL when there are findings.
    pub issue_url: Option<String>,
}

impl GuardReport {
    pub fn ok(&self) -> bool {
        self.t3_running.is_empty() && self.findings.is_empty()
    }
}

pub fn check(home: &T3Home) -> anyhow::Result<GuardReport> {
    let db = readonly(home)?;
    check_connection(home, &db)
}

#[derive(Deserialize)]
struct Known {
    objects: Vec<SchemaObject>,
}
#[derive(Deserialize)]
struct SchemaObject {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    tbl_name: String,
    sql: String,
}

/// Ignore SQL layout and keyword case, preserving quoted identifiers and literals.
fn normalized(sql: &str) -> String {
    let mut result = String::new();
    let mut quote = None;
    let mut word_gap = false;
    for c in sql.chars() {
        if let Some(q) = quote {
            result.push(c);
            if c == q {
                quote = None;
            }
        } else if matches!(c, '\'' | '"' | '`' | '[') {
            if word_gap && result.chars().last().is_some_and(|c| c.is_alphanumeric()) {
                result.push(' ');
            }
            word_gap = false;
            quote = Some(if c == '[' { ']' } else { c });
            result.push(c);
        } else if c.is_whitespace() {
            word_gap = true;
        } else {
            if word_gap
                && c.is_alphanumeric()
                && result
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                result.push(' ');
            }
            result.push(c.to_ascii_lowercase());
            word_gap = false;
        }
    }
    result
}

pub(super) fn running(home: &T3Home) -> anyhow::Result<Vec<String>> {
    #[cfg(test)]
    if let Some(inventory) = TEST_PROCESSES.with(|v| v.borrow().clone()) {
        return Ok(inventory);
    }
    let processes = System::new_all();
    let mut found = BTreeMap::new();
    for path in [
        home.userdata().join("server-runtime.json"),
        home.base.join("server-runtime.json"),
    ] {
        match std::fs::read(&path) {
            Ok(raw) => {
                let runtime: serde_json::Value = serde_json::from_slice(&raw)
                    .context("invalid server-runtime.json; cannot establish maintenance window")?;
                let pid = runtime
                    .get("pid")
                    .and_then(|p| p.as_u64())
                    .context("server-runtime.json missing pid")?;
                if processes
                    .process(Pid::from_u32(
                        pid.try_into().context("invalid runtime pid")?,
                    ))
                    .is_some()
                {
                    found.insert(pid, format!("PID {pid} recorded in server-runtime.json"));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let default_home = dirs::home_dir()
        .map(|p| p.join(".t3"))
        .map(|p| p.canonicalize().unwrap_or(p));
    for (pid, process) in processes.processes() {
        let pid = u64::from(pid.as_u32());
        if pid == u64::from(std::process::id()) {
            continue;
        }
        let argv: Vec<String> = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        if process_for_home(process.exe(), &argv, home, default_home.as_deref()) {
            // Do not include arbitrary command arguments in reports (they may contain secrets).
            found.entry(pid).or_insert_with(|| {
                format!(
                    "PID {pid}: T3 server/app/service for {}",
                    home.base.display()
                )
            });
        }
    }
    Ok(found.into_values().collect())
}

/// Custom T3CODE_HOME environments without --base-dir are detected only through
/// server-runtime.json: process environments are not read by this inventory.
fn process_for_home(
    exe: Option<&std::path::Path>,
    argv: &[String],
    home: &T3Home,
    default_home: Option<&std::path::Path>,
) -> bool {
    let executable = exe
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| argv.first().cloned())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('\\', "/");
    let name = executable.rsplit(['/', '\\']).next().unwrap_or("");
    // The installed shell launcher execs Electron with T3's bin.mjs before
    // the user's CLI arguments. Treat that as a CLI, even though exe is the app.
    let script = argv.iter().position(|arg| {
        let arg = arg.to_ascii_lowercase().replace('\\', "/");
        arg.ends_with("/apps/server/dist/bin.mjs") || arg.ends_with("/apps/server/src/bin.ts")
    });
    let cli = matches!(name, "t3" | "t3.exe") || script.is_some();
    let desktop = executable.contains("/t3 code.app/")
        || executable.contains("/t3 code (nightly).app/")
        || executable.contains("/t3code.app/")
        || matches!(
            name,
            "t3-code"
                | "t3code"
                | "t3-code.exe"
                | "t3code.exe"
                | "t3 code.exe"
                | "t3 code (nightly).exe"
        );
    let server = matches!(
        name,
        "node" | "bun" | "electron" | "node.exe" | "bun.exe" | "electron.exe"
    ) && argv.iter().skip(1).any(|arg| {
        let arg = arg.to_ascii_lowercase().replace('\\', "/");
        (arg.contains("t3code")
            || arg.contains("t3-code")
            || arg.contains("t3 code.app")
            || arg.contains("/t3 code/")
            || arg.contains("/t3/"))
            && (arg.contains("server") || arg.contains("app.asar"))
    });
    // Parse positional CLI tokens, excluding option values such as paths named "project".
    let mut tokens = Vec::new();
    let mut args = argv.iter().skip(script.map_or(1, |i| i + 1));
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "--help" | "-h" | "--version" | "-v") && cli {
            return false;
        }
        if matches!(arg.as_str(), "--base-dir" | "--host" | "--port") {
            args.next();
            continue;
        }
        if !arg.starts_with('-') {
            tokens.push(arg.as_str());
        }
    }
    let cli_server = cli
        && match tokens.as_slice() {
            [] | ["serve", ..] | ["server", ..] | ["start", ..] => true,
            [
                "service",
                "run" | "start" | "restart" | "install" | "update",
                ..,
            ] => true,
            _ => false,
        };
    if cli && !cli_server {
        return false;
    }
    if !(desktop
        || server
        || matches!(
            name,
            "t3-server" | "t3code-server" | "t3-server.exe" | "t3code-server.exe"
        )
        || cli_server)
    {
        return false;
    }
    for (i, arg) in argv.iter().enumerate() {
        let path = if arg == "--base-dir" {
            argv.get(i + 1).map(String::as_str)
        } else {
            arg.strip_prefix("--base-dir=")
        };
        if let Some(path) = path {
            return std::path::Path::new(path)
                .canonicalize()
                .unwrap_or_else(|_| path.into())
                == home.base;
        }
    }
    default_home.is_some_and(|p| p == home.base)
}

pub(super) fn check_connection(home: &T3Home, db: &Connection) -> anyhow::Result<GuardReport> {
    let mut report = GuardReport {
        t3_running: running(home)?,
        ..Default::default()
    };
    let known: Known = toml::from_str(include_str!("../../known_schemas.toml"))?;
    let expected: BTreeMap<_, _> = known
        .objects
        .into_iter()
        .map(|o| (o.name, (o.kind, o.tbl_name, normalized(&o.sql))))
        .collect();
    let mut stmt =
        db.prepare("SELECT name,type,tbl_name,coalesce(sql,'') FROM sqlite_master ORDER BY name")?;
    let actual: BTreeMap<_, _> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    normalized(&r.get::<_, String>(3)?),
                ),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(_, (_, table, _))| TABLES.contains(&table.as_str()))
        .collect();
    for name in expected
        .keys()
        .chain(actual.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if expected.get(name) != actual.get(name) {
            report.findings.push(GuardFinding {
                location: format!("sqlite_master.{name}"),
                expected: format!("{:?}", expected.get(name)),
                actual: format!("{:?}", actual.get(name)),
            });
        }
    }
    if report.findings.is_empty() {
        let version: Option<i64> = db.query_row("SELECT schema_version FROM orchestration_v2_projection_metadata WHERE projection_name='thread-projections'", [], |r| r.get(0)).optional()?;
        if version != Some(2) {
            report.findings.push(GuardFinding {
                location: "projection_metadata.schema_version".into(),
                expected: "2".into(),
                actual: format!("{version:?}"),
            });
        }
        sample_payloads(db, &mut report)?;
    }
    if !report.findings.is_empty() {
        let fingerprint = hex::encode(Sha256::digest(format!("{actual:?}").as_bytes()));
        let findings = report
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{}: expected {}; found {}",
                    f.location, f.expected, f.actual
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let body = format!(
            "T3 version: {}\nSchema fingerprint: {fingerprint}\n\n{findings}",
            t3_version(home)
        );
        report.issue_url = Some(format!(
            "https://github.com/{GITHUB_REPO}/issues/new?title={}&body={}",
            urlencoding::encode("T3 database compatibility mismatch"),
            urlencoding::encode(&body)
        ));
    }
    Ok(report)
}

fn t3_version(home: &T3Home) -> String {
    // Reading bundle metadata avoids executing the CLI during compatibility checks.
    let mut candidates = Vec::new();
    let system = System::new_all();
    let runtime_pids: Vec<u64> = [
        home.userdata().join("server-runtime.json"),
        home.base.join("server-runtime.json"),
    ]
    .iter()
    .filter_map(|p| std::fs::read(p).ok())
    .filter_map(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
    .filter_map(|v| v.get("pid").and_then(|p| p.as_u64()))
    .collect();
    let default_home = dirs::home_dir()
        .map(|p| p.join(".t3"))
        .map(|p| p.canonicalize().unwrap_or(p));
    for (pid, process) in system.processes() {
        let argv: Vec<String> = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        if runtime_pids.contains(&u64::from(pid.as_u32()))
            || process_for_home(process.exe(), &argv, home, default_home.as_deref())
        {
            if let Some(exe) = process.exe() {
                for ancestor in exe.ancestors().collect::<Vec<_>>().into_iter().rev() {
                    if ancestor.extension().is_some_and(|x| x == "app") {
                        candidates.push(ancestor.join("Contents/Info.plist"));
                    }
                }
            }
        }
    }
    if let Some(cli) = home.cli().and_then(|p| p.canonicalize().ok()) {
        for ancestor in cli.ancestors().collect::<Vec<_>>().into_iter().rev() {
            if ancestor.extension().is_some_and(|x| x == "app") {
                candidates.push(ancestor.join("Contents/Info.plist"));
            }
        }
        // T3's installed launcher is a shell file, not a symlink into the bundle.
        // Read its quoted app assignment without executing it or changing this home.
        if let Ok(text) = std::fs::read_to_string(&cli) {
            for line in text.lines() {
                if let Some(app) = line.trim().strip_prefix("app=") {
                    let app = app.trim();
                    let path = app
                        .strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                        .or_else(|| app.strip_prefix('"').and_then(|s| s.strip_suffix('"')));
                    if let Some(path) = path {
                        for ancestor in std::path::Path::new(path)
                            .ancestors()
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                        {
                            if ancestor.extension().is_some_and(|x| x == "app") {
                                candidates.push(ancestor.join("Contents/Info.plist"));
                            }
                        }
                    }
                }
            }
        }
    }
    candidates.push("/Applications/T3 Code.app/Contents/Info.plist".into());
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Some((_, tail)) = text.split_once("<key>CFBundleShortVersionString</key>") {
                if let Some((_, tail)) = tail.split_once("<string>") {
                    if let Some((version, _)) = tail.split_once("</string>") {
                        return version.to_owned();
                    }
                }
            }
        }
    }
    "unknown".into()
}

// Diagnostics describe types and field names only, never offending values or transcripts.
pub(super) fn validate(kind: &str, raw: &str) -> anyhow::Result<()> {
    fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> anyhow::Result<()> {
        serde_json::from_str::<T>(raw).map(|_| ()).map_err(|e| {
            let msg = e.to_string();
            let safe = if msg.starts_with("unknown field") || msg.starts_with("missing field") {
                msg.split(',').next().unwrap_or("field mismatch").to_owned()
            } else {
                "JSON type or shape mismatch".to_owned()
            };
            anyhow::anyhow!(safe)
        })
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| anyhow::anyhow!("invalid JSON"))?;
    if let Some(selection) = value.get("modelSelection") {
        anyhow::ensure!(
            selection
                .get("model")
                .and_then(|v| v.as_str())
                .is_some_and(|m| !m.is_empty() && m == m.trim()),
            "modelSelection.model must be nonempty and trimmed"
        );
        let routed = selection
            .get("instanceId")
            .or_else(|| selection.get("provider"));
        anyhow::ensure!(
            routed
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.trim().is_empty()),
            "modelSelection missing required instanceId/provider"
        );
    }
    match kind {
        "thread" => decode::<payload::Thread>(raw),
        "message" => decode::<payload::Message>(raw),
        "user_message" => decode::<payload::UserItem>(raw),
        "assistant_message" => decode::<payload::AssistantItem>(raw),
        "provider-thread" => decode::<payload::ProviderThread>(raw),
        "metadata" => decode::<payload::Metadata>(raw),
        "runtime" => decode::<payload::Runtime>(raw),
        "claudeAgent" => decode::<payload::ClaudeCursor>(raw),
        "codex" => decode::<payload::CodexCursor>(raw),
        "opencode" => decode::<payload::OpenCodeCursor>(raw),
        _ => bail!("unsupported payload model"),
    }
}

fn sample_payloads(db: &Connection, report: &mut GuardReport) -> anyhow::Result<()> {
    for (table, kind, filter, order) in [
        (
            "orchestration_v2_projection_threads",
            "thread",
            "1",
            "updated_at DESC",
        ),
        (
            "orchestration_v2_projection_messages",
            "message",
            "1",
            "updated_at DESC",
        ),
        (
            "orchestration_v2_projection_turn_items",
            "user_message",
            "type='user_message'",
            "updated_at DESC",
        ),
        (
            "orchestration_v2_projection_turn_items",
            "assistant_message",
            "type='assistant_message'",
            "updated_at DESC",
        ),
        (
            "orchestration_v2_projection_provider_threads",
            "provider-thread",
            "1",
            "updated_at DESC",
        ),
    ] {
        sample(
            db,
            report,
            &format!("{table}.{kind}"),
            &format!(
                "SELECT payload_json FROM {table} WHERE {filter} ORDER BY {order}, rowid DESC LIMIT 200"
            ),
            kind,
        )?;
    }
    for (event, kind) in [
        ("thread.created", "thread"),
        ("message.updated", "message"),
        ("provider-thread.updated", "provider-thread"),
    ] {
        let filter = format!("application_event_version=2 AND event_type='{event}'");
        sample(
            db,
            report,
            &format!("events.{event}.payload"),
            &format!(
                "SELECT payload_json FROM orchestration_events WHERE {filter} ORDER BY sequence DESC LIMIT 200"
            ),
            kind,
        )?;
        sample(
            db,
            report,
            &format!("events.{event}.metadata"),
            &format!(
                "SELECT metadata_json FROM orchestration_events WHERE {filter} ORDER BY sequence DESC LIMIT 200"
            ),
            "metadata",
        )?;
    }
    for kind in ["user_message", "assistant_message"] {
        let filter = format!(
            "application_event_version=2 AND event_type='turn-item.updated' AND json_extract(payload_json,'$.type')='{kind}'"
        );
        sample(
            db,
            report,
            &format!("events.turn-item.{kind}"),
            &format!(
                "SELECT payload_json FROM orchestration_events WHERE {filter} ORDER BY sequence DESC LIMIT 200"
            ),
            kind,
        )?;
    }
    sample(
        db,
        report,
        "events.turn-item.metadata",
        "SELECT metadata_json FROM orchestration_events WHERE application_event_version=2 AND event_type='turn-item.updated' ORDER BY sequence DESC LIMIT 200",
        "metadata",
    )?;
    sample(
        db,
        report,
        "runtime.payload",
        "SELECT runtime_payload_json FROM provider_session_runtime WHERE runtime_payload_json IS NOT NULL ORDER BY last_seen_at DESC LIMIT 200",
        "runtime",
    )?;
    for driver in ["claudeAgent", "codex", "opencode"] {
        sample(
            db,
            report,
            &format!("runtime.{driver}.resume"),
            &format!(
                "SELECT resume_cursor_json FROM provider_session_runtime WHERE provider_name='{driver}' AND resume_cursor_json IS NOT NULL AND resume_cursor_json!='null' ORDER BY last_seen_at DESC LIMIT 200"
            ),
            driver,
        )?;
    }
    Ok(())
}
fn sample(
    db: &Connection,
    report: &mut GuardReport,
    location: &str,
    query: &str,
    kind: &str,
) -> anyhow::Result<()> {
    let result = (|| -> rusqlite::Result<Vec<String>> {
        db.prepare(query)?.query_map([], |r| r.get(0))?.collect()
    })();
    let rows = match result {
        Ok(rows) => rows,
        Err(_) => {
            report.findings.push(GuardFinding {
                location: location.into(),
                expected: "readable JSON rows".into(),
                actual: "query failed (possibly malformed JSON)".into(),
            });
            return Ok(());
        }
    };
    report.sampled.push((location.into(), rows.len()));
    if rows.is_empty() {
        report
            .warnings
            .push(format!("No payload samples at {location}; structure only"));
    }
    for (i, row) in rows.iter().enumerate() {
        if let Err(e) = validate(kind, row) {
            report.findings.push(GuardFinding {
                location: format!("{location}.sample[{i}]"),
                expected: format!("strict {kind} encoded contract"),
                actual: e.to_string(),
            });
        }
    }
    Ok(())
}

pub(super) fn require_ok(report: GuardReport) -> anyhow::Result<()> {
    if !report.ok() {
        bail!(
            "T3 import refused. Close T3 and its background service. Processes: {}. Compatibility findings: {}. {}",
            report.t3_running.join("; "),
            report
                .findings
                .iter()
                .map(|f| format!("{}: {}", f.location, f.actual))
                .collect::<Vec<_>>()
                .join("; "),
            report.issue_url.unwrap_or_default()
        );
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    pub(super) static TEST_PROCESSES: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod process_tests {
    use super::*;
    fn matches(exe: &str, args: &[&str], home: &T3Home, default: bool) -> bool {
        let argv = std::iter::once(exe)
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        process_for_home(
            Some(std::path::Path::new(exe)),
            &argv,
            home,
            default.then_some(home.base.as_path()),
        )
    }
    #[test]
    fn detects_server_service_app_and_respects_home() {
        let home = T3Home {
            base: "/tmp/t3 test".into(),
        };
        assert!(matches(
            "/usr/local/bin/t3",
            &["service", "run", "--base-dir", "/tmp/t3 test"],
            &home,
            false
        ));
        assert!(matches(
            "/usr/bin/node",
            &[
                "/opt/t3code/apps/server/dist/index.js",
                "--base-dir=/tmp/t3 test"
            ],
            &home,
            false
        ));
        assert!(!matches(
            "/usr/local/bin/t3",
            &["service", "run", "--base-dir", "/tmp/other"],
            &home,
            false
        ));
        assert!(!matches(
            "/usr/local/bin/t3",
            &["project", "add", "/tmp/t3 test"],
            &home,
            true
        ));
        assert!(matches(
            "/usr/local/bin/t3",
            &["--base-dir", "/tmp/project settings", "service", "run"],
            &T3Home {
                base: "/tmp/project settings".into()
            },
            false
        ));
        assert!(!matches(
            "/usr/local/bin/python",
            &["script.py", "t3code server"],
            &home,
            true
        ));
        assert!(matches(
            "/Users/test/Applications/T3 Code.app/Contents/MacOS/T3 Code",
            &[],
            &home,
            true
        ));
        assert!(!matches(
            "/Users/test/Applications/T3 Code.app/Contents/MacOS/T3 Code",
            &[],
            &home,
            false
        ));
        assert!(!matches(
            "/Users/test/Applications/T3 Code.app/Contents/MacOS/T3 Code",
            &[
                "/Users/test/Applications/T3 Code.app/Contents/Resources/app.asar/apps/server/dist/bin.mjs",
                "project",
                "add",
                "/tmp/t3 test"
            ],
            &home,
            true
        ));
        assert!(matches(
            "/Users/test/Applications/T3 Code.app/Contents/MacOS/T3 Code",
            &[
                "/Users/test/Applications/T3 Code.app/Contents/Resources/app.asar/apps/server/dist/bin.mjs",
                "--base-dir",
                "/tmp/t3 test"
            ],
            &home,
            false
        ));
        assert!(!matches(
            "C:\\Apps\\t3.exe",
            &["service", "status"],
            &home,
            true
        ));
        assert!(matches(
            "C:\\Apps\\t3.exe",
            &["service", "run"],
            &home,
            true
        ));
    }

    #[cfg(unix)]
    #[test]
    fn version_prefers_this_homes_launcher_bundle() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let home = T3Home {
            base: dir.path().into(),
        };
        let app = dir.path().join("My T3 Code.app");
        std::fs::create_dir_all(app.join("Contents"))?;
        std::fs::write(
            app.join("Contents/Info.plist"),
            "<key>CFBundleShortVersionString</key><string>fixture-version</string>",
        )?;
        std::fs::create_dir(home.base.join("bin"))?;
        let cli = home.base.join("bin/t3");
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\napp='{}'\n",
                app.join("Contents/MacOS/T3 Code").display()
            ),
        )?;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700))?;
        assert_eq!(t3_version(&home), "fixture-version");
        Ok(())
    }
}
