//! Validated, optimistic trash plans. Paths always originate in scanner summaries.
use crate::{
    catalog,
    model::{Provider, SessionSummary},
    scan::Roots,
};
use anyhow::{Context, bail, ensure};
use serde::Serialize;
use std::{
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    directory: bool,
}
#[derive(Debug, Clone)]
pub struct Planned {
    pub item: Item,
    stamps: Vec<Stamp>,
    cli: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub key: String,
    pub title: String,
    pub paths: Vec<PathBuf>,
    pub export: Option<PathBuf>,
}

fn components(path: &Path, root: &Path) -> anyhow::Result<Vec<String>> {
    path.strip_prefix(root)?
        .components()
        .map(|c| match c {
            Component::Normal(s) => s
                .to_str()
                .map(str::to_owned)
                .context("non UTF-8 session path"),
            _ => bail!("invalid path component"),
        })
        .collect()
}
fn codex_layout(p: &[String]) -> bool {
    let file = p
        .last()
        .is_some_and(|f| f.starts_with("rollout-") && f.ends_with(".jsonl"));
    file && ((p.len() == 5
        && p[0] == "sessions"
        && p[1].len() == 4
        && p[2].len() == 2
        && p[3].len() == 2
        && p[1..4]
            .iter()
            .all(|s| s.bytes().all(|b| b.is_ascii_digit())))
        || (p.len() >= 2 && p[0] == "archived_sessions"))
}
fn validate(root: &Path, target: &Path) -> anyhow::Result<Vec<Stamp>> {
    components(target, root)?;
    let mut current = root.to_path_buf();
    ensure!(
        !std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink(),
        "symlink provider root"
    );
    for part in target.strip_prefix(root)?.components() {
        current.push(part);
        ensure!(
            !std::fs::symlink_metadata(&current)?
                .file_type()
                .is_symlink(),
            "symlink target: {}",
            current.display()
        );
    }
    ensure!(
        target.canonicalize()?.starts_with(root.canonicalize()?),
        "target outside provider root"
    );
    let mut stamps = Vec::new();
    for entry in walkdir::WalkDir::new(target).follow_links(false) {
        let entry = entry?;
        let meta = std::fs::symlink_metadata(entry.path())?;
        ensure!(
            !meta.file_type().is_symlink(),
            "symlink target: {}",
            entry.path().display()
        );
        ensure!(meta.is_file() || meta.is_dir(), "unsupported target type");
        stamps.push(Stamp {
            path: entry.path().into(),
            size: meta.len(),
            modified: meta.modified()?,
            directory: meta.is_dir(),
        });
    }
    stamps.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(stamps)
}
pub fn opencode_cli() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.extend([home.join(".opencode/bin"), home.join(".local/bin")]);
    }
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    find_cli(&dirs)
}
fn find_cli(dirs: &[PathBuf]) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["opencode.exe", "opencode.cmd", "opencode"]
    } else {
        &["opencode"]
    };
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| {
            let Ok(m) = p.metadata() else {
                return false;
            };
            if !m.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
}
fn uuid_shaped(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn codex_filename(name: &str, id: &str) -> bool {
    if !uuid_shaped(id) {
        return false;
    }
    let Some(timestamp) = name
        .strip_prefix("rollout-")
        .and_then(|s| s.strip_suffix(&format!("-{id}.jsonl")))
    else {
        return false;
    };
    // Codex uses an ISO timestamp with either ':' or '-' time separators.
    let normalized = timestamp
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if [13, 16].contains(&i) && c == '-' {
                ':'
            } else {
                c
            }
        })
        .collect::<String>();
    chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S%.f").is_ok()
        || chrono::DateTime::parse_from_rfc3339(&normalized).is_ok()
}
fn plan_opencode(roots: &Roots, s: &SessionSummary, cli: PathBuf) -> anyhow::Result<Planned> {
    let mut item = Item {
        key: catalog::key(s),
        title: s.title.clone(),
        paths: vec![],
        export: None,
    };
    let children = crate::scan::opencode::child_count(&roots.opencode, s)?;
    ensure!(
        children == 0,
        "has {children} sub-agent sessions; delete it from OpenCode"
    );
    item.export = Some(
        dirs::data_dir()
            .context("cannot determine data directory")?
            .join("t3-import/opencode-exports")
            .join(format!(
                "{}-{}.json",
                s.session_id,
                chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ")
            )),
    );
    Ok(Planned {
        item,
        stamps: vec![],
        cli: Some(cli),
    })
}
pub fn plan(roots: &Roots, s: &SessionSummary) -> anyhow::Result<Planned> {
    let mut item = Item {
        key: catalog::key(s),
        title: s.title.clone(),
        paths: vec![],
        export: None,
    };
    if s.provider == Provider::OpenCode {
        ensure!(
            s.session_id.starts_with("ses_")
                && s.session_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "invalid OpenCode session id: expected ses_ prefix and safe characters"
        );
        return plan_opencode(
            roots,
            s,
            opencode_cli()
                .context("OpenCode CLI not found in PATH or common install directories")?,
        );
    }
    let root = match s.provider {
        Provider::Claude => &roots.claude,
        Provider::Codex => &roots.codex,
        Provider::Grok => &roots.grok,
        _ => unreachable!(),
    };
    let parts = components(&s.source, root).context("source outside provider layout")?;
    match s.provider {
        Provider::Claude => {
            ensure!(
                parts.len() == 3
                    && parts[0] == "projects"
                    && s.source.extension().is_some_and(|e| e == "jsonl")
                    && s.source.is_file(),
                "invalid Claude layout"
            );
            item.paths.push(s.source.clone());
            let sibling = s.source.with_extension("");
            if std::fs::symlink_metadata(&sibling).is_ok() {
                ensure!(sibling.is_dir(), "Claude sibling is not a directory");
                item.paths.insert(0, sibling);
            }
        }
        Provider::Codex => {
            ensure!(
                codex_layout(&parts) && s.source.is_file(),
                "invalid Codex layout"
            );
            ensure!(
                uuid_shaped(&s.session_id),
                "invalid Codex session id: expected a UUID"
            );
            ensure!(
                codex_filename(parts.last().unwrap(), &s.session_id),
                "Codex filename must be rollout-<timestamp>-<session_id>.jsonl"
            );
            item.paths.push(s.source.clone());
            for tree in ["sessions", "archived_sessions"] {
                let tree = root.join(tree);
                if !tree.exists() {
                    continue;
                }
                for entry in walkdir::WalkDir::new(tree).follow_links(false) {
                    let entry = entry?;
                    let p = entry.path();
                    if !entry.file_type().is_dir()
                        && codex_filename(&entry.file_name().to_string_lossy(), &s.session_id)
                        && codex_layout(&components(p, root)?)
                    {
                        item.paths.push(p.into());
                    }
                }
            }
        }
        Provider::Grok => {
            ensure!(
                parts.len() == 3 && parts[0] == "sessions" && s.source.is_dir(),
                "invalid Grok layout"
            );
            item.paths.push(s.source.clone());
        }
        _ => unreachable!(),
    }
    if s.provider == Provider::Codex {
        item.paths.sort();
    }
    let mut seen = std::collections::HashSet::new();
    item.paths.retain(|p| seen.insert(p.clone()));
    let mut stamps = vec![];
    for p in &item.paths {
        stamps.extend(validate(root, p)?);
    }
    Ok(Planned {
        item,
        stamps,
        cli: None,
    })
}
pub fn unchanged(roots: &Roots, s: &SessionSummary, old: &Planned) -> anyhow::Result<()> {
    let fresh = plan(roots, s)?;
    ensure!(
        fresh.item.paths == old.item.paths && fresh.stamps == old.stamps && fresh.cli == old.cli,
        "Session files changed since the trash plan; review a new plan"
    );
    Ok(())
}
pub fn execute(
    roots: &Roots,
    s: &SessionSummary,
    old: &Planned,
    exported: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    unchanged(roots, s, old)?;
    if let Some(cli) = &old.cli {
        export_and_delete(
            cli,
            &s.session_id,
            s.message_count,
            old.item.export.as_deref().context("missing export path")?,
            exported,
        )?;
    } else {
        trash_targets(&old.item.paths, |path| Ok(::trash::delete(path)?))?;
    }
    Ok(())
}
fn trash_targets(
    paths: &[PathBuf],
    mut delete: impl FnMut(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut moved = Vec::new();
    for path in paths {
        if let Err(error) = delete(path) {
            bail!(
                "Failed to trash {}: {error:#}; targets moved: {}",
                path.display(),
                if moved.is_empty() {
                    "none".into()
                } else {
                    moved.join(", ")
                }
            );
        }
        moved.push(path.to_string_lossy().into_owned());
    }
    Ok(())
}
fn export_count(json: &serde_json::Value) -> anyhow::Result<usize> {
    let messages = json
        .get("messages")
        .and_then(|v| v.as_array())
        .context("OpenCode export missing messages array")?;
    // Summary counts user/assistant messages with nonempty text parts, not tool-only rows.
    Ok(messages
        .iter()
        .filter(|m| {
            matches!(
                m.pointer("/info/role").and_then(|v| v.as_str()),
                Some("user" | "assistant")
            ) && m
                .get("parts")
                .and_then(|p| p.as_array())
                .is_some_and(|parts| {
                    parts.iter().any(|p| {
                        p.get("type").and_then(|v| v.as_str()) == Some("text")
                            && p.get("text")
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| !s.trim().is_empty())
                    })
                })
        })
        .count())
}

fn export_and_delete(
    cli: &Path,
    session_id: &str,
    message_count: Option<usize>,
    path: &Path,
    exported: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().context("missing export directory")?)?;
    let file = private_file(path)?;
    // A regular file avoids OpenCode's truncated piped stdout on large sessions.
    let result = (|| -> anyhow::Result<()> {
        let status = export_to(cli, session_id, file.try_clone()?, message_count)?;
        ensure!(status.success(), "OpenCode export failed: {status}");
        file.sync_all()?;
        let json: serde_json::Value = serde_json::from_reader(std::fs::File::open(path)?)
            .context("invalid OpenCode export JSON")?;
        ensure!(
            json.pointer("/info/id").and_then(|v| v.as_str()) == Some(session_id),
            "OpenCode export info.id does not equal session id"
        );
        let actual = export_count(&json)?;
        if let Some(expected) = message_count {
            ensure!(
                actual == expected,
                "OpenCode export message count mismatch: expected {expected}, got {actual}"
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    exported.push(path.to_path_buf());
    let out = crate::t3db::control::output_timeout(
        Command::new(cli).args(["session", "delete", session_id]),
        Duration::from_secs(30),
    )?;
    ensure!(
        out.status.success(),
        "OpenCode delete failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A successful export means the session may still exist. Fail conservatively;
    // no stdout pipe is used for this check either.
    let verification = wait_command(
        Command::new(cli)
            .args(["export", session_id])
            .stdout(Stdio::null()),
        export_timeout(message_count),
    )?;
    ensure!(
        !verification.success(),
        "OpenCode delete failed: session still exports; backup kept at {}",
        path.display()
    );
    Ok(())
}
fn private_file(path: &Path) -> anyhow::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn export_timeout(message_count: Option<usize>) -> Duration {
    Duration::from_secs(120 + (message_count.unwrap_or(0) as u64 / 100).saturating_mul(10))
}
fn export_to(
    cli: &Path,
    id: &str,
    file: std::fs::File,
    count: Option<usize>,
) -> anyhow::Result<std::process::ExitStatus> {
    wait_command(
        Command::new(cli).args(["export", id]).stdout(file),
        export_timeout(count),
    )
}
fn wait_command(cmd: &mut Command, timeout: Duration) -> anyhow::Result<std::process::ExitStatus> {
    let mut child = cmd.stdin(Stdio::null()).stderr(Stdio::null()).spawn()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                if let Err(error) = result {
                    return Err(error.into());
                }
                bail!("OpenCode didn't finish within {}s", timeout.as_secs());
            }
        }
    }
}

pub fn recently_used(s: &SessionSummary) -> bool {
    s.source
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .is_some_and(|m| {
            SystemTime::now().duration_since(m).unwrap_or_default() < Duration::from_secs(120)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roots(p: &Path) -> Roots {
        Roots {
            claude: p.join("claude"),
            codex: p.join("codex"),
            grok: p.join("grok"),
            opencode: p.join("opencode"),
        }
    }
    fn session(provider: Provider, source: PathBuf) -> SessionSummary {
        SessionSummary {
            provider,
            source,
            session_id: if provider == Provider::Codex {
                "12345678-1234-1234-1234-123456789abc"
            } else if provider == Provider::OpenCode {
                "ses_test"
            } else {
                "session-id"
            }
            .into(),
            cwd: "/cwd".into(),
            title: "test".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            message_count: None,
            model: None,
            resumable: true,
        }
    }
    fn file(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "test").unwrap();
    }
    #[test]
    fn unknown_opencode_children_block_deletion() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let mut s = session(Provider::OpenCode, r.opencode.join("opencode.db"));
        let cli = PathBuf::from("/fake/opencode");
        assert!(
            plan_opencode(&r, &s, cli.clone())
                .unwrap_err()
                .to_string()
                .contains("can't tell whether")
        );
        std::fs::create_dir_all(&r.opencode).unwrap();
        let conn = rusqlite::Connection::open(&s.source).unwrap();
        conn.execute_batch("CREATE TABLE session (id TEXT);")
            .unwrap();
        assert!(plan_opencode(&r, &s, cli.clone()).is_err());
        drop(conn);
        std::fs::write(&s.source, "invalid database").unwrap();
        assert!(plan_opencode(&r, &s, cli.clone()).is_err());
        s.source = r.opencode.join("storage/session/project/ses_test.json");
        assert!(
            plan_opencode(&r, &s, cli)
                .unwrap_err()
                .to_string()
                .contains("can't tell whether it has sub-agent sessions; delete it from OpenCode")
        );
    }
    #[cfg(unix)]
    #[test]
    fn large_export_uses_private_regular_file_and_checks_argv() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let payload = d.path().join("payload.json");
        let backup = d.path().join("backup.json");
        let marker = d.path().join("deleted");
        let cli = d.path().join("opencode");
        let json = serde_json::json!({"info":{"id":"ses_test"},"messages":[{"info":{"role":"user"},"parts":[{"type":"text","text":"x".repeat(512 * 1024)}]}]});
        std::fs::write(&payload, serde_json::to_vec(&json).unwrap()).unwrap();
        std::fs::write(
            &cli,
            format!(
                r#"#!/bin/sh
if [ "$1" = export ]; then
  [ "$#" = 2 ] && [ "$2" = ses_test ] || exit 99
  [ -f '{}' ] && exit 1
  [ -f /dev/stdout ] || exit 98
  cat '{}'
else
  [ "$#" = 3 ] && [ "$1 $2 $3" = "session delete ses_test" ] || exit 99
  touch '{}'
fi
"#,
                marker.display(),
                payload.display(),
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut exported = vec![];
        export_and_delete(&cli, "ses_test", Some(1), &backup, &mut exported).unwrap();
        assert_eq!(
            std::fs::read(&backup).unwrap(),
            std::fs::read(payload).unwrap()
        );
        assert_eq!(
            backup.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(exported, vec![backup.clone()]);
        assert!(export_and_delete(&cli, "ses_test", Some(1), &backup, &mut vec![]).is_err());
        assert!(export_timeout(Some(10000)) > export_timeout(None));
    }
    #[cfg(unix)]
    #[test]
    fn direct_export_wait_has_a_time_limit() {
        let start = Instant::now();
        let error =
            wait_command(Command::new("sleep").arg("30"), Duration::from_millis(100)).unwrap_err();
        assert!(error.to_string().contains("didn't finish"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    #[test]
    fn claude_sibling_and_changed_file() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let p = r.claude.join("projects/group/session-id.jsonl");
        file(&p);
        std::fs::create_dir(p.with_extension("")).unwrap();
        let s = session(Provider::Claude, p.clone());
        let plan = plan(&r, &s).unwrap();
        assert_eq!(plan.item.paths, vec![p.with_extension(""), p.clone()]);
        assert!(unchanged(&r, &s, &plan).is_ok());
        std::fs::write(p, "changed length").unwrap();
        assert!(unchanged(&r, &s, &plan).is_err());
    }
    #[test]
    fn codex_archived_duplicates() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let p = r.codex.join("sessions/2026/10/14/rollout-2026-10-14T12-00-00-12345678-1234-1234-1234-123456789abc.jsonl");
        file(&p);
        let archived = r.codex.join("archived_sessions/rollout-2026-10-14T12-00-00-12345678-1234-1234-1234-123456789abc.jsonl");
        file(&archived);
        let s = session(Provider::Codex, p.clone());
        let planned = plan(&r, &s).unwrap();
        assert_eq!(planned.item.paths, vec![archived, p]);
        assert!(unchanged(&r, &s, &planned).is_ok());
    }
    #[test]
    fn grok_never_targets_group_and_invalid_layouts_fail() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let p = r.grok.join("sessions/group/session");
        std::fs::create_dir_all(&p).unwrap();
        assert_eq!(
            plan(&r, &session(Provider::Grok, p.clone()))
                .unwrap()
                .item
                .paths,
            vec![p.clone()]
        );
        assert!(plan(&r, &session(Provider::Grok, p.parent().unwrap().into())).is_err());
        let p = r.claude.join("wrong/file.jsonl");
        file(&p);
        assert!(plan(&r, &session(Provider::Claude, p)).is_err());
        let p = d.path().join("outside.jsonl");
        file(&p);
        assert!(plan(&r, &session(Provider::Claude, p)).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let p = r.claude.join("projects/group/session.jsonl");
        file(&p);
        let link = p.with_file_name("link.jsonl");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(plan(&r, &session(Provider::Claude, link)).is_err());
        let s = session(Provider::Claude, p.clone());
        let old = plan(&r, &s).unwrap();
        let other = p.with_file_name("other.jsonl");
        file(&other);
        std::fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(other, &p).unwrap();
        assert!(unchanged(&r, &s, &old).is_err());
    }
    #[test]
    fn opencode_has_no_file_targets() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let cli = d.path().join("bin").join(if cfg!(windows) {
            "opencode.exe"
        } else {
            "opencode"
        });
        file(&cli);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // Explicit search dirs keep this test independent of installed CLIs.
        let cli = find_cli(&[d.path().join("bin")]).unwrap();
        std::fs::create_dir_all(&r.opencode).unwrap();
        let conn = rusqlite::Connection::open(r.opencode.join("opencode.db")).unwrap();
        conn.execute_batch("CREATE TABLE session (id TEXT, parent_id TEXT);")
            .unwrap();
        let s = session(Provider::OpenCode, r.opencode.join("opencode.db"));
        let p = plan_opencode(&r, &s, cli).unwrap();
        assert!(p.item.paths.is_empty());
        assert_eq!(p.item.export.unwrap().extension().unwrap(), "json");
    }
    #[cfg(unix)]
    #[test]
    fn opencode_exports_before_delete_and_rejects_invalid_output() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join("opencode");
        let backup = d.path().join("exports/session.json");
        let marker = d.path().join("deleted");
        let mut exported = vec![];
        for output in ["not-json", r#"{"id":"wrong-session"}"#] {
            std::fs::write(&cli, format!("#!/bin/sh\nif [ \"$1\" = export ]; then [ \"$#\" = 2 ] && [ \"$2\" = ses_test ] || exit 99; else [ \"$#\" = 3 ] && [ \"$1 $2 $3\" = \"session delete ses_test\" ] || exit 99; fi\nif [ \"$1\" = export ]; then\n  echo '{}'; exit 0\nfi\ntouch '{}'\n", output, marker.display())).unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(export_and_delete(&cli, "ses_test", None, &backup, &mut exported).is_err());
            assert!(!marker.exists());
            assert!(!backup.exists());
        }
        std::fs::write(&cli, format!("#!/bin/sh\nif [ \"$1\" = export ]; then [ \"$#\" = 2 ] && [ \"$2\" = ses_test ] || exit 99; else [ \"$#\" = 3 ] && [ \"$1 $2 $3\" = \"session delete ses_test\" ] || exit 99; fi\nif [ \"$1\" = export ]; then\n  echo '{{\"info\":{{\"id\":\"ses_test\"}},\"messages\":[]}}'; exit 0\nfi\n[ -s '{}' ] || exit 3\ntouch '{}'\nexit 1\n", backup.display(), marker.display())).unwrap();
        assert!(export_and_delete(&cli, "ses_test", None, &backup, &mut exported).is_err());
        assert!(marker.exists());
        assert_eq!(exported, vec![backup.clone()]);
        assert!(backup.is_file());
    }
    #[test]
    fn codex_exact_suffix_and_uuid_required() {
        let id = "12345678-1234-1234-1234-123456789abc";
        assert!(codex_filename(
            &format!("rollout-2026-10-14T12-00-00-{id}.jsonl"),
            id
        ));
        for name in [
            format!("rollout-2026-10-14T12-00-00-{id}-extra.jsonl"),
            format!("rollout-junk-{id}.jsonl"),
        ] {
            assert!(!codex_filename(&name, id));
        }
        assert!(!codex_filename(
            "rollout-2026-10-14T12-00-00-other.jsonl",
            "2026"
        ));
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let p = r
            .codex
            .join("sessions/2026/10/14/rollout-2026-unrelated.jsonl");
        file(&p);
        let mut s = session(Provider::Codex, p);
        s.session_id = "2026".into();
        assert!(plan(&r, &s).unwrap_err().to_string().contains("UUID"));
    }
    #[test]
    fn codex_and_grok_changes_refused() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let p = r.codex.join("sessions/2026/10/14/rollout-2026-10-14T12-00-00-12345678-1234-1234-1234-123456789abc.jsonl");
        file(&p);
        let s = session(Provider::Codex, p.clone());
        let old = plan(&r, &s).unwrap();
        std::fs::write(p, "changed contents").unwrap();
        assert!(unchanged(&r, &s, &old).is_err());
        let p = r.grok.join("sessions/group/session");
        std::fs::create_dir_all(&p).unwrap();
        let s = session(Provider::Grok, p.clone());
        let old = plan(&r, &s).unwrap();
        file(&p.join("new-file"));
        assert!(unchanged(&r, &s, &old).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn claude_symlinked_sibling_refused() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        let p = r.claude.join("projects/group/session.jsonl");
        file(&p);
        std::os::unix::fs::symlink(d.path(), p.with_extension("")).unwrap();
        assert!(
            plan(&r, &session(Provider::Claude, p))
                .unwrap_err()
                .to_string()
                .contains("symlink")
        );
    }
    #[cfg(unix)]
    #[test]
    fn nonzero_export_aborts_delete() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join("opencode");
        let marker = d.path().join("deleted");
        std::fs::write(&cli, format!("#!/bin/sh\nif [ \"$1\" = export ]; then [ \"$#\" = 2 ] && [ \"$2\" = ses_test ] || exit 99; else [ \"$#\" = 3 ] && [ \"$1 $2 $3\" = \"session delete ses_test\" ] || exit 99; fi\nif [ \"$1\" = export ]; then echo '{{\"info\":{{\"id\":\"ses_test\"}}}}'; exit 1; fi\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        // A child-local temporary PATH avoids mutating process-wide env in parallel tests.
        let mut command = Command::new("/bin/sh");
        command
            .env("PATH", d.path())
            .args(["-c", "command -v opencode"]);
        let found = command.output().unwrap();
        assert!(found.status.success());
        let found = PathBuf::from(String::from_utf8(found.stdout).unwrap().trim());
        let mut exported = vec![];
        assert!(
            export_and_delete(
                &found,
                "ses_test",
                None,
                &d.path().join("backup.json"),
                &mut exported
            )
            .unwrap_err()
            .to_string()
            .contains("export failed")
        );
        assert!(!marker.exists());
        assert!(exported.is_empty());
    }
    #[test]
    fn partial_trash_reports_moved_targets_in_order() {
        let paths = vec![PathBuf::from("sibling"), PathBuf::from("session.jsonl")];
        let mut seen = vec![];
        let error = trash_targets(&paths, |p| {
            seen.push(p.to_path_buf());
            if seen.len() == 2 {
                bail!("failed");
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(seen, paths);
        assert!(error.to_string().contains("targets moved: sibling"));
    }
    #[test]
    fn opencode_children_block_parent_and_revalidate() {
        let d = tempfile::tempdir().unwrap();
        let r = roots(d.path());
        std::fs::create_dir_all(&r.opencode).unwrap();
        let db = r.opencode.join("opencode.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE session (id TEXT, parent_id TEXT); INSERT INTO session VALUES ('ses_parent', NULL);").unwrap();
        let mut s = session(Provider::OpenCode, db);
        s.session_id = "ses_parent".into();
        let cli = PathBuf::from("/fake/opencode");
        let old = plan_opencode(&r, &s, cli.clone()).unwrap();
        assert!(old.item.paths.is_empty());
        conn.execute_batch("INSERT INTO session VALUES ('ses_child', 'ses_parent');")
            .unwrap();
        assert!(
            plan_opencode(&r, &s, cli)
                .unwrap_err()
                .to_string()
                .contains("has 1 sub-agent sessions; delete it from OpenCode")
        );
    }
    #[test]
    fn opencode_counts_text_messages_like_scanner() {
        let json = serde_json::json!({"messages":[
            {"info":{"role":"user"},"parts":[{"type":"text","text":"hello"},{"type":"text","text":"again"}]},
            {"info":{"role":"assistant"},"parts":[{"type":"tool","text":"ignored"}]},
            {"info":{"role":"assistant"},"parts":[{"type":"text","text":" "}]},
            {"info":{"role":"assistant"},"parts":[{"type":"text","text":"reply"}]}
        ]});
        assert_eq!(export_count(&json).unwrap(), 2);
        assert!(export_count(&serde_json::json!({})).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn opencode_verifies_deletion_and_keeps_backup() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join("opencode");
        for (label, after, succeeds) in [
            (
                "still-there",
                "echo '{\"info\":{\"id\":\"ses_test\"},\"messages\":[]}'",
                false,
            ),
            ("gone", "exit 1", true),
        ] {
            let marker = d.path().join(label);
            std::fs::write(&cli, format!("#!/bin/sh\nif [ \"$1\" = export ]; then [ \"$#\" = 2 ] && [ \"$2\" = ses_test ] || exit 99; else [ \"$#\" = 3 ] && [ \"$1 $2 $3\" = \"session delete ses_test\" ] || exit 99; fi\nif [ \"$1\" = export ]; then\n  if [ -f '{}' ]; then {}; else echo '{{\"info\":{{\"id\":\"ses_test\"}},\"messages\":[]}}'; fi\nelse touch '{}'; fi\n", marker.display(), after, marker.display())).unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
            let backup = d.path().join(format!("{label}.json"));
            let mut exported = vec![];
            assert_eq!(
                export_and_delete(&cli, "ses_test", Some(0), &backup, &mut exported).is_ok(),
                succeeds
            );
            assert!(backup.is_file());
            assert_eq!(exported, vec![backup]);
        }
    }
    #[cfg(unix)]
    #[test]
    fn opencode_wrong_info_or_count_aborts_before_backup_and_delete() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join("opencode");
        let marker = d.path().join("deleted");
        let backup = d.path().join("backup.json");
        for json in [
            serde_json::json!({"info":{"id":"ses_other"},"messages":[],"mentioned":"ses_test"}),
            serde_json::json!({"info":{"id":"ses_test"},"messages":[]}),
        ] {
            std::fs::write(
                &cli,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = export ]; then [ \"$#\" = 2 ] && [ \"$2\" = ses_test ] || exit 99; else [ \"$#\" = 3 ] && [ \"$1 $2 $3\" = \"session delete ses_test\" ] || exit 99; fi\nif [ \"$1\" = export ]; then echo '{}'; else touch '{}'; fi\n",
                    json,
                    marker.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
            let mut exported = vec![];
            assert!(export_and_delete(&cli, "ses_test", Some(1), &backup, &mut exported).is_err());
            assert!(!marker.exists());
            assert!(!backup.exists());
            assert!(exported.is_empty());
        }
    }
}
