use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

fn open_readonly(path: &Path) -> anyhow::Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let primary = sqlite_uri(path, "mode=ro");
    match Connection::open_with_flags(&primary, flags) {
        Ok(conn) => {
            let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
            let _ = conn.pragma_update(None, "query_only", "ON");
            Ok(conn)
        }
        Err(error) => {
            let fallback = sqlite_uri(path, "mode=ro&immutable=1");
            Connection::open_with_flags(&fallback, flags).map_err(|fallback_error| {
                anyhow::anyhow!(
                    "open {}: {error}; immutable: {fallback_error}",
                    path.display()
                )
            })
        }
    }
}

fn sqlite_uri(path: &Path, query: &str) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut encoded = String::new();
    for ch in absolute.to_string_lossy().chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3f"),
            '#' => encoded.push_str("%23"),
            '\\' => encoded.push('/'),
            other => encoded.push(other),
        }
    }
    format!("file:{encoded}?{query}")
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
    let conn = open_readonly(path)?;
    let session_cols = columns(&conn, "session");
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
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let counts = message_counts(&conn);
    let mut sessions = Vec::new();
    for row in rows.flatten() {
        let (id, directory, title, created, updated, model) = row;
        let Some(id) = id.filter(|id| !id.trim().is_empty()) else {
            continue;
        };
        let directory = directory.unwrap_or_default();
        let created = created.unwrap_or(0);
        let updated = updated.unwrap_or(created);
        let recorded = clean_recorded_title(title.as_deref().unwrap_or(""));
        let title = match recorded {
            Some(title) => title,
            None => first_user_text(&conn, &id)
                .map(|text| title_from_text(&text))
                .unwrap_or_else(|| "Untitled".to_string()),
        };
        let model = model
            .as_deref()
            .and_then(normalize_model)
            .or_else(|| last_message_model(&conn, &id));
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

fn message_counts(conn: &Connection) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    let sql = "
        SELECT m.session_id, COUNT(DISTINCT m.id)
        FROM message m
        JOIN part p ON p.message_id = m.id
        WHERE json_extract(p.data, '$.type') = 'text'
          AND length(trim(COALESCE(json_extract(p.data, '$.text'), ''))) > 0
          AND json_extract(m.data, '$.role') IN ('user', 'assistant')
        GROUP BY m.session_id
    ";
    let Ok(mut statement) = conn.prepare(sql) else {
        return counts;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return counts;
    };
    for row in rows.flatten() {
        counts.insert(row.0, row.1.max(0) as usize);
    }
    counts
}

fn first_user_text(conn: &Connection, session_id: &str) -> Option<String> {
    conn.query_row(
        "
        SELECT json_extract(p.data, '$.text')
        FROM message m
        JOIN part p ON p.message_id = m.id
        WHERE m.session_id = ?1
          AND json_extract(m.data, '$.role') = 'user'
          AND json_extract(p.data, '$.type') = 'text'
          AND length(trim(COALESCE(json_extract(p.data, '$.text'), ''))) > 0
        ORDER BY m.time_created, m.id, p.time_created, p.id
        LIMIT 1
        ",
        [session_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

fn last_message_model(conn: &Connection, session_id: &str) -> Option<String> {
    let model: String = conn
        .query_row(
            "
            SELECT json_extract(data, '$.modelID')
            FROM message
            WHERE session_id = ?1
              AND json_extract(data, '$.role') = 'assistant'
              AND json_extract(data, '$.modelID') IS NOT NULL
            ORDER BY time_created DESC, id DESC
            LIMIT 1
            ",
            [session_id],
            |row| row.get(0),
        )
        .ok()?;
    let model = model.trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
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
    let conn = open_readonly(path)?;
    let mut messages_meta = Vec::new();
    {
        let mut statement = conn.prepare(
            "
            SELECT id, time_created, json_extract(data, '$.role')
            FROM message
            WHERE session_id = ?1
            ORDER BY time_created, id
            ",
        )?;
        let rows = statement.query_map([&summary.session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows.flatten() {
            messages_meta.push(row);
        }
    }
    let mut parts: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut statement = conn.prepare(
            "
            SELECT message_id, json_extract(data, '$.type'), json_extract(data, '$.text')
            FROM part
            WHERE session_id = ?1
            ORDER BY time_created, id
            ",
        )?;
        let rows = statement.query_map([&summary.session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows.flatten() {
            let (message_id, kind, text) = row;
            if kind.as_deref() != Some("text") {
                continue;
            }
            let Some(text) = text else { continue };
            if text.trim().is_empty() {
                continue;
            }
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
        model = last_message_model(&conn, &summary.session_id);
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
}
