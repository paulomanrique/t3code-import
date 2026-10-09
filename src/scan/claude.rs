use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde_json::Value;

use crate::model::{Message, Provider, Role, SessionSummary, Transcript};

use super::Scanner;
use super::util::{
    clean_recorded_title, extract_text, file_mtime, json_string_first, json_string_last,
    parse_rfc3339, plausible_model, title_from_text,
};

pub struct ClaudeScanner {
    root: PathBuf,
}

impl ClaudeScanner {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Scanner for ClaudeScanner {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn scan(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let files = discover(&self.root.join("projects"));
        let mut sessions: Vec<SessionSummary> = files
            .par_iter()
            .filter_map(|path| scan_file(path))
            .collect();
        dedupe_newer(&mut sessions);
        Ok(sessions)
    }

    fn load(&self, summary: &SessionSummary) -> anyhow::Result<Transcript> {
        load_file(&summary.source, summary)
            .with_context(|| format!("load claude session {}", summary.session_id))
    }
}

/// Top-level `projects/<slug>/*.jsonl` only. Subagent transcripts live under
/// `<session>/subagents/` and are not resumable conversations.
fn discover(projects: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(projects) = std::fs::read_dir(projects) else {
        return files;
    };
    for project in projects.flatten() {
        let Ok(kind) = project.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files
}

struct Header {
    cwd: Option<PathBuf>,
    first_user: Option<String>,
    created: Option<DateTime<Utc>>,
}

fn scan_file(path: &Path) -> Option<SessionSummary> {
    let id = path.file_stem().and_then(|stem| stem.to_str())?.to_string();
    let file = File::open(path).ok()?;
    let mtime = file_mtime(path);
    let mut reader = BufReader::with_capacity(128 * 1024, file);
    let mut raw = Vec::new();
    let mut header = Header {
        cwd: None,
        first_user: None,
        created: None,
    };
    let mut custom_title = None;
    let mut ai_title = None;
    let mut model = None;

    loop {
        raw.clear();
        let read = reader.read_until(b'\n', &mut raw).ok()?;
        if read == 0 {
            break;
        }
        if raw.last() == Some(&b'\n') {
            raw.pop();
        }
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        if raw.is_empty() {
            continue;
        }
        let Ok(line) = std::str::from_utf8(&raw) else {
            continue;
        };

        if let Some(title) =
            json_string_last(line, "customTitle").and_then(|v| clean_recorded_title(&v))
        {
            custom_title = Some(title);
        }
        if let Some(title) =
            json_string_last(line, "aiTitle").and_then(|v| clean_recorded_title(&v))
        {
            ai_title = Some(title);
        }
        if line.contains("\"role\":\"assistant\"") || line.contains("\"type\":\"assistant\"") {
            if let Some(found) = json_string_first(line, "model") {
                if plausible_model(&found) {
                    model = Some(found);
                }
            }
        }
        if header.created.is_none() && line.contains("\"timestamp\"") {
            if let Some(ts) = json_string_first(line, "timestamp") {
                header.created = parse_rfc3339(&ts);
            }
        }
        if header.cwd.is_none() || header.first_user.is_none() {
            // Tool-result rows are type=user but carry no visible text. Once the
            // working directory is known, skip them so a large result is not parsed.
            let tool_only = header.cwd.is_some()
                && line.contains("\"tool_result\"")
                && !line.contains("\"type\":\"text\"")
                && !line.contains("\"type\": \"text\"");
            let interesting = !tool_only
                && (line.contains("\"cwd\"")
                    || line.contains("\"type\":\"user\"")
                    || line.contains("\"type\": \"user\""));
            if interesting {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    absorb_header(&mut header, &value);
                }
            }
        }
    }

    let title = custom_title.or(ai_title).unwrap_or_else(|| {
        header
            .first_user
            .as_deref()
            .map(title_from_text)
            .unwrap_or_else(|| "Untitled".to_string())
    });
    Some(SessionSummary {
        provider: Provider::Claude,
        session_id: id.clone(),
        cwd: header.cwd.unwrap_or_default(),
        title,
        created_at: header.created.unwrap_or(mtime),
        updated_at: mtime,
        message_count: None,
        model,
        source: path.to_path_buf(),
        resumable: super::util::is_uuid(&id),
    })
}

fn absorb_header(header: &mut Header, record: &Value) {
    if header.cwd.is_none() {
        if let Some(cwd) = record.get("cwd").and_then(Value::as_str) {
            let cwd = cwd.trim();
            if !cwd.is_empty() {
                header.cwd = Some(PathBuf::from(cwd));
            }
        }
    }
    if header.created.is_none() {
        if let Some(ts) = record.get("timestamp").and_then(Value::as_str) {
            header.created = parse_rfc3339(ts);
        }
    }
    if header.first_user.is_some() || skipped_record(record) {
        return;
    }
    if record.get("type").and_then(Value::as_str) != Some("user") {
        return;
    }
    let Some(content) = record
        .get("message")
        .and_then(|message| message.get("content"))
    else {
        return;
    };
    let text = extract_text(content);
    if !text.is_empty() {
        header.first_user = Some(text);
    }
}

fn skipped_record(record: &Value) -> bool {
    flag(record, "isSidechain") || flag(record, "isMeta") || flag(record, "isCompactSummary")
}

fn flag(record: &Value, key: &str) -> bool {
    record.get(key).and_then(Value::as_bool) == Some(true)
}

fn load_file(path: &Path, summary: &SessionSummary) -> anyhow::Result<Transcript> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mtime = file_mtime(path);
    let reader = BufReader::with_capacity(128 * 1024, file);
    let mut summary = summary.clone();
    let mut messages = Vec::new();
    let mut custom_title = None;
    let mut ai_title = None;
    let mut first_user = None;
    let mut created = None;
    let mut cwd = None;
    let mut last_at = summary.created_at;

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if cwd.is_none() {
            if let Some(value) = record.get("cwd").and_then(Value::as_str) {
                let value = value.trim();
                if !value.is_empty() {
                    cwd = Some(PathBuf::from(value));
                }
            }
        }
        if let Some(ts) = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339)
        {
            if created.is_none() {
                created = Some(ts);
            }
            last_at = ts;
        }
        if skipped_record(&record) {
            continue;
        }
        if let Some(title) = record
            .get("customTitle")
            .and_then(Value::as_str)
            .and_then(clean_recorded_title)
        {
            custom_title = Some(title);
        }
        if let Some(title) = record
            .get("aiTitle")
            .and_then(Value::as_str)
            .and_then(clean_recorded_title)
        {
            ai_title = Some(title);
        }
        let Some(kind) = record.get("type").and_then(Value::as_str) else {
            continue;
        };
        let role = match kind {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => continue,
        };
        if role == Role::Assistant {
            if let Some(found) = record.pointer("/message/model").and_then(Value::as_str) {
                if plausible_model(found) {
                    summary.model = Some(found.to_string());
                }
            }
        }
        let Some(content) = record
            .get("message")
            .and_then(|message| message.get("content"))
        else {
            continue;
        };
        let text = extract_text(content);
        if text.is_empty() {
            continue;
        }
        if first_user.is_none() && role == Role::User {
            first_user = Some(text.clone());
        }
        let at = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339)
            .unwrap_or(last_at);
        messages.push(Message {
            role,
            text,
            created_at: at,
        });
    }

    if let Some(cwd) = cwd {
        summary.cwd = cwd;
    }
    summary.title = custom_title.or(ai_title).unwrap_or_else(|| {
        first_user
            .as_deref()
            .map(title_from_text)
            .unwrap_or_else(|| "Untitled".to_string())
    });
    summary.created_at = created.unwrap_or(summary.created_at);
    summary.updated_at = mtime;
    summary.message_count = Some(messages.len());
    Ok(Transcript { summary, messages })
}

fn dedupe_newer(sessions: &mut Vec<SessionSummary>) {
    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    let mut seen = std::collections::HashSet::new();
    sessions.retain(|session| seen.insert(session.session_id.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ClaudeScanner {
        ClaudeScanner::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude"))
    }

    fn by_id(sessions: &[SessionSummary], id: &str) -> SessionSummary {
        sessions
            .iter()
            .find(|session| session.session_id == id)
            .cloned()
            .unwrap_or_else(|| panic!("missing {id}"))
    }

    #[test]
    fn scans_metadata_and_skips_subagents() {
        let sessions = fixture().scan().unwrap();
        let ids: Vec<_> = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert!(ids.contains(&"11111111-1111-4111-8111-111111111111"));
        assert!(ids.contains(&"88888888-8888-4888-8888-888888888888"));
        assert!(ids.contains(&"99999999-9999-4999-8999-999999999999"));
        assert!(ids.contains(&"scratch"));
        assert!(!ids.iter().any(|id| id.contains("agent")));

        let main = by_id(&sessions, "11111111-1111-4111-8111-111111111111");
        assert_eq!(main.cwd, PathBuf::from("/tmp/demo"));
        assert_eq!(main.title, "Fix authentication");
        assert_eq!(main.model.as_deref(), Some("claude-sonnet-5"));
        assert!(main.resumable);
        assert!(main.message_count.is_none());

        let renamed = by_id(&sessions, "99999999-9999-4999-8999-999999999999");
        assert_eq!(renamed.title, "Manual name");

        let long = by_id(&sessions, "88888888-8888-4888-8888-888888888888");
        assert_eq!(long.title, "a".repeat(80));
        assert_eq!(long.title.chars().count(), 80);

        let scratch = by_id(&sessions, "scratch");
        assert!(!scratch.resumable);
        assert_eq!(scratch.cwd, PathBuf::from("/tmp/scratch"));
    }

    #[test]
    fn loads_visible_text_only() {
        let scanner = fixture();
        let summary = by_id(
            &scanner.scan().unwrap(),
            "11111111-1111-4111-8111-111111111111",
        );
        let transcript = scanner.load(&summary).unwrap();
        let texts: Vec<_> = transcript
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "Fix authentication please"),
                (Role::Assistant, "Updated the login flow"),
                (Role::Assistant, "The provider request failed"),
            ]
        );
        assert_eq!(transcript.summary.message_count, Some(3));
        assert_eq!(transcript.summary.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(transcript.summary.title, "Fix authentication");
        assert_eq!(
            transcript.messages[0].created_at,
            parse_rfc3339("2026-08-24T10:00:02.000Z").unwrap()
        );
    }

    #[test]
    fn missing_home_is_empty() {
        let sessions = ClaudeScanner::new(PathBuf::from("/no/such/claude-home"))
            .scan()
            .unwrap();
        assert!(sessions.is_empty());
    }
}
