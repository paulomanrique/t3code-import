use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::model::{Message, Provider, Role, SessionSummary, Transcript};

use super::Scanner;
use super::util::{clean_recorded_title, file_mtime, title_from_text, unix_to_utc};

pub struct OpenCodeScanner {
    root: PathBuf,
}

impl OpenCodeScanner {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Scanner for OpenCodeScanner {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn scan(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let mut sessions = Vec::new();
        let mut seen = HashSet::new();
        if let Some(db_path) = db_path(&self.root) {
            if let Ok(found) = scan_db(&db_path) {
                for session in found {
                    seen.insert(session.session_id.clone());
                    sessions.push(session);
                }
            }
        }
        for session in scan_storage(&self.root.join("storage"), &seen) {
            sessions.push(session);
        }
        Ok(sessions)
    }

    fn load(&self, summary: &SessionSummary) -> anyhow::Result<Transcript> {
        if summary.source.file_name().and_then(|name| name.to_str()) == Some("opencode.db") {
            load_db(&summary.source, summary)
                .with_context(|| format!("load opencode session {}", summary.session_id))
        } else {
            load_storage(&summary.source, summary)
                .with_context(|| format!("load opencode session {}", summary.session_id))
        }
    }
}

fn db_path(root: &Path) -> Option<PathBuf> {
    let path = root.join("opencode.db");
    path.is_file().then_some(path)
}

/// Owns the private copy of an OpenCode database. The connection is closed
/// before the directory is removed.
struct SnapshotDb {
    conn: Option<Connection>,
    dir: PathBuf,
}

impl SnapshotDb {
    fn connection(&self) -> &Connection {
        self.conn.as_ref().expect("snapshot connection open")
    }
}

impl Drop for SnapshotDb {
    fn drop(&mut self) {
        self.conn.take();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Copy `opencode.db` and `opencode.db-wal` (if present) once, then open the
/// copy. A read-only open of a live WAL database still creates and updates
/// `opencode.db-shm` next to the real file; the copy keeps uncheckpointed
/// frames and those writes stay in the temp directory.
fn snapshot_db(src: &Path) -> anyhow::Result<SnapshotDb> {
    let dir = unique_snapshot_dir()?;
    match copy_and_open(src, &dir) {
        Ok(conn) => Ok(SnapshotDb {
            conn: Some(conn),
            dir,
        }),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(error)
        }
    }
}

fn unique_snapshot_dir() -> anyhow::Result<PathBuf> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    for _ in 0..1000 {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("t3-import-opencode-{}-{n}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("create {}", dir.display()));
            }
        }
    }
    anyhow::bail!("could not create a private opencode snapshot directory")
}

fn copy_and_open(src: &Path, dir: &Path) -> anyhow::Result<Connection> {
    let file_name = src.file_name().context("opencode database file name")?;
    let dest = dir.join(file_name);
    std::fs::copy(src, &dest).with_context(|| format!("copy {}", src.display()))?;
    let mut wal_name = file_name.to_os_string();
    wal_name.push("-wal");
    let wal_src = src.with_file_name(&wal_name);
    if wal_src.is_file() {
        std::fs::copy(&wal_src, dir.join(&wal_name))
            .with_context(|| format!("copy {}", wal_src.display()))?;
    }
    open_readonly(&dest)
}

/// Opens the private copy made by the caller. Plain path, no URI: SQLite may
/// create its `-shm` next to the copy, which is ours to throw away.
fn open_readonly(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", path.display()))?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    let _ = conn.pragma_update(None, "query_only", "ON");
    Ok(conn)
}

#[cfg(test)]
fn sqlite_uri(path: &Path, query: &str) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut raw = absolute.to_string_lossy().replace('\\', "/");
    // Windows verbatim prefix from canonicalize: \\?\C:\x -> C:/x
    if let Some(rest) = raw.strip_prefix("//?/") {
        raw = rest.to_string();
    }
    let mut encoded = String::new();
    for ch in raw.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3f"),
            '#' => encoded.push_str("%23"),
            other => encoded.push(other),
        }
    }
    if !encoded.starts_with('/') {
        encoded.insert(0, '/'); // drive letter: file:///C:/x
    }
    format!("file://{encoded}?{query}")
}

fn columns(conn: &Connection, table: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let Ok(mut statement) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return names;
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(1)) else {
        return names;
    };
    for name in rows.flatten() {
        names.insert(name);
    }
    names
}

fn scan_db(path: &Path) -> anyhow::Result<Vec<SessionSummary>> {
    let db = snapshot_db(path)?;
    let conn = db.connection();
    let session_cols = columns(conn, "session");
    if !session_cols.contains("id") || !session_cols.contains("directory") {
        return Ok(Vec::new());
    }
    let model_sql = if session_cols.contains("model") {
        "model"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT id, directory, title, time_created, time_updated, {model_sql} FROM session"
    );
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query([])?;
    let mut raw_rows = Vec::new();
    while let Some(row) = rows.next()? {
        raw_rows.push((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ));
    }
    drop(rows);
    drop(statement);
    let counts = message_counts(conn)?;
    let mut sessions = Vec::new();
    for (id, directory, title, created, updated, model) in raw_rows {
        let Some(id) = id.filter(|id| !id.trim().is_empty()) else {
            continue;
        };
        let directory = directory.unwrap_or_default();
        let created = created.unwrap_or(0);
        let updated = updated.unwrap_or(created);
        let recorded = clean_recorded_title(title.as_deref().unwrap_or(""));
        let title = match recorded {
            Some(title) => title,
            None => match first_user_text(conn, &id)? {
                Some(text) => title_from_text(&text),
                None => "Untitled".to_string(),
            },
        };
        let model = match model.as_deref().and_then(normalize_model) {
            Some(model) => Some(model),
            None => last_message_model(conn, &id)?,
        };
        sessions.push(SessionSummary {
            provider: Provider::OpenCode,
            session_id: id.clone(),
            cwd: PathBuf::from(directory),
            title,
            created_at: unix_to_utc(created),
            updated_at: unix_to_utc(updated),
            message_count: counts.get(&id).copied(),
            model,
            source: path.to_path_buf(),
            resumable: true,
        });
    }
    Ok(sessions)
}

fn message_counts(conn: &Connection) -> anyhow::Result<HashMap<String, usize>> {
    let mut statement = conn.prepare(
        "
        SELECT m.session_id, m.id, m.data, p.data
        FROM message m
        JOIN part p ON p.message_id = m.id
        ",
    )?;
    let mut rows = statement.query([])?;
    let mut seen: HashMap<String, HashSet<String>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let message_id: String = row.get(1)?;
        let message_data: String = row.get(2)?;
        let part_data: String = row.get(3)?;
        let Some(message) = parse_data(&message_data) else {
            continue;
        };
        let Some(part) = parse_data(&part_data) else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str);
        if role != Some("user") && role != Some("assistant") {
            continue;
        }
        if text_part(&part).is_none() {
            continue;
        }
        seen.entry(session_id).or_default().insert(message_id);
    }
    Ok(seen
        .into_iter()
        .map(|(session_id, ids)| (session_id, ids.len()))
        .collect())
}

fn first_user_text(conn: &Connection, session_id: &str) -> anyhow::Result<Option<String>> {
    let mut statement = conn.prepare(
        "
        SELECT m.data, p.data
        FROM message m
        JOIN part p ON p.message_id = m.id
        WHERE m.session_id = ?1
        ORDER BY m.time_created, m.id, p.time_created, p.id
        ",
    )?;
    let mut rows = statement.query([session_id])?;
    while let Some(row) = rows.next()? {
        let message_data: String = row.get(0)?;
        let part_data: String = row.get(1)?;
        let Some(message) = parse_data(&message_data) else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(part) = parse_data(&part_data) else {
            continue;
        };
        if let Some(text) = text_part(&part) {
            return Ok(Some(text));
        }
    }
    Ok(None)
}

fn last_message_model(conn: &Connection, session_id: &str) -> anyhow::Result<Option<String>> {
    let mut statement = conn.prepare(
        "
        SELECT data
        FROM message
        WHERE session_id = ?1
        ORDER BY time_created DESC, id DESC
        ",
    )?;
    let mut rows = statement.query([session_id])?;
    while let Some(row) = rows.next()? {
        let data: String = row.get(0)?;
        let Some(value) = parse_data(&data) else {
            continue;
        };
        if value.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(model) = value.get("modelID").and_then(Value::as_str) else {
            continue;
        };
        let model = model.trim();
        if !model.is_empty() {
            return Ok(Some(model.to_string()));
        }
    }
    Ok(None)
}

fn parse_data(raw: &str) -> Option<Value> {
    serde_json::from_str(raw).ok()
}

fn text_part(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    let text = value.get("text").and_then(Value::as_str)?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn normalize_model(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('{') {
        let value: Value = serde_json::from_str(raw).ok()?;
        let id = value.get("id").and_then(Value::as_str)?.trim();
        if id.is_empty() {
            None
        } else {
            Some(id.to_string())
        }
    } else {
        Some(raw.to_string())
    }
}

fn load_db(path: &Path, summary: &SessionSummary) -> anyhow::Result<Transcript> {
    let db = snapshot_db(path)?;
    let conn = db.connection();
    let mut messages_meta = Vec::new();
    {
        let mut statement = conn.prepare(
            "
            SELECT id, time_created, data
            FROM message
            WHERE session_id = ?1
            ORDER BY time_created, id
            ",
        )?;
        let mut rows = statement.query([&summary.session_id])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let created: i64 = row.get(1)?;
            let data: String = row.get(2)?;
            let Some(value) = parse_data(&data) else {
                continue;
            };
            let role = value
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string);
            messages_meta.push((id, created, role));
        }
    }
    let mut parts: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut statement = conn.prepare(
            "
            SELECT message_id, data
            FROM part
            WHERE session_id = ?1
            ORDER BY time_created, id
            ",
        )?;
        let mut rows = statement.query([&summary.session_id])?;
        while let Some(row) = rows.next()? {
            let message_id: String = row.get(0)?;
            let data: String = row.get(1)?;
            let Some(value) = parse_data(&data) else {
                continue;
            };
            let Some(text) = text_part(&value) else {
                continue;
            };
            parts.entry(message_id).or_default().push(text);
        }
    }
    let mut messages = Vec::new();
    let mut model = summary.model.clone();
    for (id, created, role) in messages_meta {
        let role = match role.as_deref() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };
        let Some(chunks) = parts.get(&id) else {
            continue;
        };
        let text = chunks.join("\n");
        if text.trim().is_empty() {
            continue;
        }
        messages.push(Message {
            role,
            text,
            created_at: unix_to_utc(created),
        });
    }
    if model.is_none() {
        model = last_message_model(conn, &summary.session_id)?;
    }
    let mut summary = summary.clone();
    if summary.title.trim().is_empty() || summary.title == "Untitled" {
        if let Some(text) = messages.iter().find(|message| message.role == Role::User) {
            summary.title = title_from_text(&text.text);
        }
    }
    summary.model = model;
    summary.message_count = Some(messages.len());
    Ok(Transcript { summary, messages })
}

fn scan_storage(storage: &Path, seen: &HashSet<String>) -> Vec<SessionSummary> {
    let mut sessions = Vec::new();
    let session_root = storage.join("session");
    let Ok(projects) = std::fs::read_dir(&session_root) else {
        return sessions;
    };
    for project in projects.flatten() {
        let Ok(kind) = project.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(session) = scan_storage_session(&path, storage) else {
                continue;
            };
            if seen.contains(&session.session_id) {
                continue;
            }
            sessions.push(session);
        }
    }
    sessions
}

fn scan_storage_session(path: &Path, storage: &Path) -> Option<SessionSummary> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    let directory = value.get("directory").and_then(Value::as_str).unwrap_or("");
    let created = value
        .pointer("/time/created")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let updated = value
        .pointer("/time/updated")
        .and_then(Value::as_i64)
        .unwrap_or(created);
    let messages = storage_messages(storage, id, unix_to_utc(created));
    let recorded = value
        .get("title")
        .and_then(Value::as_str)
        .and_then(clean_recorded_title);
    let title = recorded.unwrap_or_else(|| {
        messages
            .iter()
            .find(|message| message.role == Role::User)
            .map(|message| title_from_text(&message.text))
            .unwrap_or_else(|| "Untitled".to_string())
    });
    let model = messages_model(storage, id);
    let mtime = file_mtime(path);
    Some(SessionSummary {
        provider: Provider::OpenCode,
        session_id: id.to_string(),
        cwd: PathBuf::from(directory),
        title,
        created_at: if created == 0 {
            mtime
        } else {
            unix_to_utc(created)
        },
        updated_at: if updated == 0 {
            mtime
        } else {
            unix_to_utc(updated)
        },
        message_count: Some(messages.len()),
        model,
        source: path.to_path_buf(),
        resumable: true,
    })
}

fn load_storage(path: &Path, summary: &SessionSummary) -> anyhow::Result<Transcript> {
    let storage = path
        .parent()
        .and_then(|parent| parent.parent())
        .and_then(|parent| parent.parent())
        .context("opencode storage session path")?
        .to_path_buf();
    // storage/session/<project>/<id>.json → storage is three parents up.
    let messages = storage_messages(&storage, &summary.session_id, summary.created_at);
    let mut summary = summary.clone();
    summary.message_count = Some(messages.len());
    if summary.model.is_none() {
        summary.model = messages_model(&storage, &summary.session_id);
    }
    if let Some(text) = messages.iter().find(|message| message.role == Role::User) {
        if summary.title == "Untitled" {
            summary.title = title_from_text(&text.text);
        }
    }
    Ok(Transcript { summary, messages })
}

fn storage_messages(storage: &Path, session_id: &str, fallback: DateTime<Utc>) -> Vec<Message> {
    let message_dir = storage.join("message").join(session_id);
    let Ok(entries) = std::fs::read_dir(&message_dir) else {
        return Vec::new();
    };
    let mut metas = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        let role = value
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let created = value
            .pointer("/time/created")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        metas.push((created, id, role));
    }
    metas.sort();
    let mut messages = Vec::new();
    for (created, id, role) in metas {
        let role = match role.as_str() {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => continue,
        };
        let text = storage_part_text(storage, &id);
        if text.trim().is_empty() {
            continue;
        }
        let at = if created == 0 {
            fallback
        } else {
            unix_to_utc(created)
        };
        messages.push(Message {
            role,
            text,
            created_at: at,
        });
    }
    messages
}

fn storage_part_text(storage: &Path, message_id: &str) -> String {
    let dir = storage.join("part").join(message_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(body) = value.get("text").and_then(Value::as_str) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        let order = value
            .pointer("/time/start")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_string();
        parts.push((order, name, body.to_string()));
    }
    parts.sort();
    parts
        .into_iter()
        .map(|(_, _, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn messages_model(storage: &Path, session_id: &str) -> Option<String> {
    let message_dir = storage.join("message").join(session_id);
    let entries = std::fs::read_dir(&message_dir).ok()?;
    let mut found = None;
    let mut best = i64::MIN;
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(model) = value.get("modelID").and_then(Value::as_str) else {
            continue;
        };
        if model.trim().is_empty() {
            continue;
        }
        let created = value
            .pointer("/time/created")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if created >= best {
            best = created;
            found = Some(model.to_string());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> OpenCodeScanner {
        OpenCodeScanner::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opencode"),
        )
    }

    fn by_id(sessions: &[SessionSummary], id: &str) -> SessionSummary {
        sessions
            .iter()
            .find(|session| session.session_id == id)
            .cloned()
            .unwrap_or_else(|| panic!("missing {id}"))
    }

    #[test]
    fn scans_database_and_legacy_storage() {
        let sessions = fixture().scan().unwrap();
        let ids: Vec<_> = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert_eq!(ids.iter().filter(|id| **id == "ses_db1").count(), 1);
        assert!(ids.contains(&"ses_notitle"));
        assert!(ids.contains(&"ses_legacy"));

        let db = by_id(&sessions, "ses_db1");
        assert_eq!(db.title, "Database session");
        assert_eq!(db.cwd, PathBuf::from("/tmp/opencode-db"));
        assert_eq!(db.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(db.message_count, Some(2));
        assert!(db.resumable);
        assert!(db.source.ends_with("opencode.db"));

        let untitled = by_id(&sessions, "ses_notitle");
        assert_eq!(untitled.title, "Name this session from the prompt");
        assert_eq!(untitled.model.as_deref(), Some("muse-spark"));
        assert_eq!(untitled.message_count, Some(1));

        let legacy = by_id(&sessions, "ses_legacy");
        assert_eq!(legacy.title, "Legacy storage session");
        assert_eq!(legacy.cwd, PathBuf::from("/tmp/opencode-legacy"));
        assert_eq!(legacy.message_count, Some(2));
        assert!(legacy.source.ends_with("ses_legacy.json"));
    }

    #[test]
    fn loads_text_parts_only() {
        let scanner = fixture();
        let sessions = scanner.scan().unwrap();
        let db = scanner.load(&by_id(&sessions, "ses_db1")).unwrap();
        let texts: Vec<_> = db
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "Hello from the database"),
                (Role::Assistant, "Hi there")
            ]
        );
        assert_eq!(db.messages[0].created_at, unix_to_utc(1_700_000_001_000));

        let legacy = scanner.load(&by_id(&sessions, "ses_legacy")).unwrap();
        let texts: Vec<_> = legacy
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "Hello from storage"),
                (Role::Assistant, "Stored reply")
            ]
        );
    }

    #[test]
    fn missing_home_is_empty() {
        assert!(
            OpenCodeScanner::new(PathBuf::from("/no/such/opencode-home"))
                .scan()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn skips_malformed_json_without_dropping_later_rows() {
        let scanner = OpenCodeScanner::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opencode-bad-json"),
        );
        let sessions = scanner.scan().unwrap();
        let summary = by_id(&sessions, "ses_badjson");
        assert_eq!(summary.message_count, Some(3));
        assert_eq!(summary.model.as_deref(), Some("after-model"));
        assert_eq!(summary.title, "Malformed rows");
        let transcript = scanner.load(&summary).unwrap();
        let texts: Vec<_> = transcript
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "kept before"),
                (Role::Assistant, "kept after"),
                (Role::User, "second prompt"),
            ]
        );
    }

    #[test]
    fn reads_uncheckpointed_wal_without_writing_beside_the_source() {
        let root =
            std::env::temp_dir().join(format!("t3-import-opencode-wal-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("opencode.db");
        let scratch = root.join("scratch.db");
        {
            let conn = Connection::open(&scratch).unwrap();
            conn.execute_batch(
                "
                PRAGMA journal_mode=WAL;
                PRAGMA wal_autocheckpoint=0;
                CREATE TABLE session (
                  id TEXT PRIMARY KEY,
                  directory TEXT NOT NULL,
                  title TEXT NOT NULL,
                  time_created INTEGER NOT NULL,
                  time_updated INTEGER NOT NULL,
                  model TEXT
                );
                CREATE TABLE message (
                  id TEXT PRIMARY KEY,
                  session_id TEXT NOT NULL,
                  time_created INTEGER NOT NULL,
                  data TEXT NOT NULL
                );
                CREATE TABLE part (
                  id TEXT PRIMARY KEY,
                  session_id TEXT NOT NULL,
                  message_id TEXT NOT NULL,
                  time_created INTEGER NOT NULL,
                  data TEXT NOT NULL
                );
                INSERT INTO session VALUES (
                  'ses_base', '/tmp/wal', 'Base', 1, 1, NULL
                );
                INSERT INTO message VALUES (
                  'msg_base', 'ses_base', 1, '{\"role\":\"user\"}'
                );
                INSERT INTO part VALUES (
                  'prt_base', 'ses_base', 'msg_base', 1,
                  '{\"type\":\"text\",\"text\":\"already checkpointed\"}'
                );
                ",
            )
            .unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
            conn.execute_batch(
                "
                INSERT INTO session VALUES (
                  'ses_wal', '/tmp/wal', 'Only in the wal', 2, 2, NULL
                );
                INSERT INTO message VALUES (
                  'msg_wal', 'ses_wal', 2, '{\"role\":\"user\"}'
                );
                INSERT INTO part VALUES (
                  'prt_wal', 'ses_wal', 'msg_wal', 2,
                  '{\"type\":\"text\",\"text\":\"uncheckpointed reply\"}'
                );
                ",
            )
            .unwrap();
            std::fs::copy(&scratch, &source).unwrap();
            let wal = scratch.with_file_name("scratch.db-wal");
            assert!(wal.is_file());
            assert!(wal.metadata().unwrap().len() > 0);
            std::fs::copy(&wal, root.join("opencode.db-wal")).unwrap();
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
        let immutable =
            Connection::open_with_flags(sqlite_uri(&source, "mode=ro&immutable=1"), flags).unwrap();
        let visible: i64 = immutable
            .query_row(
                "SELECT COUNT(*) FROM session WHERE id = 'ses_wal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(visible, 0, "ses_wal must live only in the copied wal");
        drop(immutable);

        let before = std::fs::read(&source).unwrap();
        let wal_before = std::fs::read(root.join("opencode.db-wal")).unwrap();
        assert!(!root.join("opencode.db-shm").exists());

        let sessions = scan_db(&source).unwrap();
        assert!(
            sessions
                .iter()
                .any(|session| session.session_id == "ses_wal")
        );
        assert!(
            sessions
                .iter()
                .any(|session| session.session_id == "ses_base")
        );
        let summary = sessions
            .iter()
            .find(|session| session.session_id == "ses_wal")
            .unwrap();
        assert_eq!(summary.message_count, Some(1));
        let transcript = load_db(&source, summary).unwrap();
        assert_eq!(transcript.messages.len(), 1);
        assert_eq!(transcript.messages[0].text, "uncheckpointed reply");

        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(
            std::fs::read(root.join("opencode.db-wal")).unwrap(),
            wal_before
        );
        assert!(!root.join("opencode.db-shm").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
