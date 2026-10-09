use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use crate::model::{Message, Provider, Role, SessionSummary, Transcript};

use super::Scanner;
use super::util::{
    clean_recorded_title, file_mtime, is_uuid, json_string_first, parse_rfc3339, title_from_text,
    unix_to_utc,
};

pub struct GrokScanner {
    root: PathBuf,
}

impl GrokScanner {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Scanner for GrokScanner {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    fn scan(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let dirs = discover(&self.root.join("sessions"));
        let mut sessions: Vec<SessionSummary> =
            dirs.par_iter().filter_map(|dir| scan_dir(dir)).collect();
        dedupe_newer(&mut sessions);
        Ok(sessions)
    }

    fn load(&self, summary: &SessionSummary) -> anyhow::Result<Transcript> {
        load_dir(&summary.source, summary)
            .with_context(|| format!("load grok session {}", summary.session_id))
    }
}

fn discover(sessions: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(groups) = std::fs::read_dir(sessions) else {
        return found;
    };
    for group in groups.flatten() {
        let Ok(kind) = group.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let Ok(children) = std::fs::read_dir(group.path()) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            let Ok(kind) = child.file_type() else {
                continue;
            };
            if !kind.is_dir()
                || path.file_name().and_then(|name| name.to_str()) == Some("subagents")
            {
                continue;
            }
            if path.join("summary.json").is_file()
                || path.join("updates.jsonl").is_file()
                || path.join("chat_history.jsonl").is_file()
            {
                found.push(path);
            }
        }
    }
    found
}

#[derive(Deserialize)]
struct SummaryFile {
    info: Option<SummaryInfo>,
    session_summary: Option<String>,
    generated_title: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    last_active_at: Option<String>,
    current_model_id: Option<String>,
}

#[derive(Deserialize)]
struct SummaryInfo {
    id: Option<String>,
    cwd: Option<String>,
}

fn scan_dir(dir: &Path) -> Option<SessionSummary> {
    let summary = read_summary(dir);
    let id = summary
        .as_ref()
        .and_then(|summary| summary.info.as_ref())
        .and_then(|info| info.id.as_deref())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })?;
    let cwd = cwd_of(dir, summary.as_ref());
    let recorded = summary.as_ref().and_then(|summary| {
        clean_recorded_title(summary.generated_title.as_deref().unwrap_or(""))
            .or_else(|| clean_recorded_title(summary.session_summary.as_deref().unwrap_or("")))
    });
    let title = recorded.unwrap_or_else(|| {
        first_user_text(dir)
            .map(|text| title_from_text(&text))
            .unwrap_or_else(|| "Untitled".to_string())
    });
    let created = summary
        .as_ref()
        .and_then(|summary| summary.created_at.as_deref().and_then(parse_rfc3339));
    let updated = summary.as_ref().and_then(|summary| {
        summary
            .updated_at
            .as_deref()
            .and_then(parse_rfc3339)
            .or_else(|| summary.last_active_at.as_deref().and_then(parse_rfc3339))
    });
    let mtime = file_mtime(dir);
    let model = summary
        .as_ref()
        .and_then(|summary| summary.current_model_id.as_deref())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string);
    Some(SessionSummary {
        provider: Provider::Grok,
        session_id: id.clone(),
        cwd,
        title,
        created_at: created.unwrap_or(mtime),
        updated_at: updated.unwrap_or(mtime),
        message_count: None,
        model,
        source: dir.to_path_buf(),
        resumable: is_uuid(&id),
    })
}

fn read_summary(dir: &Path) -> Option<SummaryFile> {
    let text = std::fs::read_to_string(dir.join("summary.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn cwd_of(dir: &Path, summary: Option<&SummaryFile>) -> PathBuf {
    if let Some(cwd) = summary
        .and_then(|summary| summary.info.as_ref())
        .and_then(|info| info.cwd.as_deref())
    {
        let cwd = cwd.trim();
        if !cwd.is_empty() {
            return PathBuf::from(cwd);
        }
    }
    if let Some(parent) = dir.parent() {
        if let Ok(text) = std::fs::read_to_string(parent.join(".cwd")) {
            let text = text.trim();
            if !text.is_empty() {
                return PathBuf::from(text);
            }
        }
        if let Some(name) = parent.file_name().and_then(|name| name.to_str()) {
            return PathBuf::from(
                urlencoding::decode(name)
                    .unwrap_or(std::borrow::Cow::Borrowed(name))
                    .as_ref(),
            );
        }
    }
    PathBuf::new()
}

fn load_dir(dir: &Path, summary: &SessionSummary) -> anyhow::Result<Transcript> {
    let mut summary = summary.clone();
    let messages = select_messages(dir, summary.created_at);
    if summary.title == "Untitled" || summary.title.is_empty() {
        if let Some(text) = messages
            .iter()
            .find(|message| message.role == Role::User)
            .map(|message| title_from_text(&message.text))
        {
            summary.title = text;
        }
    }
    if let Some(model) = last_chat_model(&dir.join("chat_history.jsonl")) {
        if summary.model.is_none() {
            summary.model = Some(model);
        }
    }
    summary.message_count = Some(messages.len());
    if let Some(first) = messages.first() {
        if summary.created_at == DateTime::UNIX_EPOCH {
            summary.created_at = first.created_at;
        }
    }
    Ok(Transcript { summary, messages })
}

enum Updates {
    Missing,
    Incomplete,
    Messages(Vec<Message>),
}

/// `updates.jsonl` wins ties. A read error is incomplete, so `chat_history.jsonl`
/// is used even when the updates file exists. A shorter updates file (a prompt
/// with no reply yet) loses to the longer chat history.
fn select_messages(dir: &Path, fallback: DateTime<Utc>) -> Vec<Message> {
    let chat_path = dir.join("chat_history.jsonl");
    match messages_from_updates(&dir.join("updates.jsonl")) {
        Updates::Missing | Updates::Incomplete => messages_from_chat(&chat_path, fallback),
        Updates::Messages(updates) => {
            if !chat_path.is_file() {
                return updates;
            }
            let chat = messages_from_chat(&chat_path, fallback);
            if dialogue_count(&chat) > dialogue_count(&updates) {
                chat
            } else {
                updates
            }
        }
    }
}

fn dialogue_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| matches!(message.role, Role::User | Role::Assistant))
        .count()
}

fn messages_from_updates(path: &Path) -> Updates {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Updates::Missing,
        Err(_) => return Updates::Incomplete,
    };
    match updates_from_reader(BufReader::new(file)) {
        Ok(messages) => Updates::Messages(messages),
        Err(_) => Updates::Incomplete,
    }
}

fn updates_from_reader(reader: impl BufRead) -> std::io::Result<Vec<Message>> {
    let mut messages = Vec::new();
    let mut pending_user: Option<(String, DateTime<Utc>)> = None;
    let mut pending_assistant: Option<(String, DateTime<Utc>)> = None;
    let flush_user = |pending: &mut Option<(String, DateTime<Utc>)>,
                      messages: &mut Vec<Message>| {
        if let Some((text, at)) = pending.take() {
            push_user(messages, &text, at);
        }
    };
    let flush_assistant = |pending: &mut Option<(String, DateTime<Utc>)>,
                           messages: &mut Vec<Message>| {
        if let Some((text, at)) = pending.take() {
            let text = text.trim();
            if !text.is_empty() {
                messages.push(Message {
                    role: Role::Assistant,
                    text: text.to_string(),
                    created_at: at,
                });
            }
        }
    };
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let kind = json_string_first(&line, "sessionUpdate").unwrap_or_default();
        let at = line_timestamp(&line).unwrap_or(DateTime::UNIX_EPOCH);
        match kind.as_str() {
            "user_message_chunk" => {
                flush_assistant(&mut pending_assistant, &mut messages);
                let text = chunk_text(&line);
                match &mut pending_user {
                    Some((existing, _)) => existing.push_str(&text),
                    None => pending_user = Some((text, at)),
                }
            }
            "agent_message_chunk" => {
                flush_user(&mut pending_user, &mut messages);
                let text = chunk_text(&line);
                match &mut pending_assistant {
                    Some((existing, _)) => existing.push_str(&text),
                    None => pending_assistant = Some((text, at)),
                }
            }
            _ => {
                flush_user(&mut pending_user, &mut messages);
                flush_assistant(&mut pending_assistant, &mut messages);
            }
        }
    }
    flush_user(&mut pending_user, &mut messages);
    flush_assistant(&mut pending_assistant, &mut messages);
    Ok(messages)
}

fn messages_from_chat(path: &Path, fallback: DateTime<Utc>) -> Vec<Message> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut messages = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let kind = record.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "user" => {
                if record
                    .get("synthetic_reason")
                    .and_then(Value::as_str)
                    .is_some()
                {
                    continue;
                }
                let text = content_text(record.get("content"));
                push_user(&mut messages, &text, fallback);
            }
            "assistant" => {
                let text = record
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if text.is_empty() {
                    continue;
                }
                messages.push(Message {
                    role: Role::Assistant,
                    text: text.to_string(),
                    created_at: fallback,
                });
            }
            _ => {}
        }
    }
    messages
}

fn first_user_text(dir: &Path) -> Option<String> {
    select_messages(dir, DateTime::UNIX_EPOCH)
        .into_iter()
        .find(|message| message.role == Role::User)
        .map(|message| message.text)
}

fn last_chat_model(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut model = None;
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if !line.contains("\"model_id\"") {
            continue;
        }
        if let Some(found) = json_string_first(&line, "model_id") {
            if !found.trim().is_empty() {
                model = Some(found);
            }
        }
    }
    model
}

fn line_timestamp(line: &str) -> Option<DateTime<Utc>> {
    let key = "\"timestamp\":";
    let index = line.find(key)?;
    let rest = line[index + key.len()..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let raw: i64 = digits.parse().ok()?;
    Some(unix_to_utc(raw))
}

fn chunk_text(line: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return String::new();
    };
    let content = value.pointer("/params/update/content");
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(map)) => map
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    return None;
                }
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// `updates.jsonl` user chunks are the raw prompt. `chat_history.jsonl` wraps the
/// same prompt in `<user_query>` and also stores `<user_info>` rows that are not
/// something the user typed.
fn push_user(messages: &mut Vec<Message>, text: &str, at: DateTime<Utc>) {
    let queries = user_queries(text);
    let visible = if !queries.is_empty() {
        queries.join("\n")
    } else if injected(text) {
        return;
    } else {
        text.trim().to_string()
    };
    if visible.is_empty() {
        return;
    }
    messages.push(Message {
        role: Role::User,
        text: visible,
        created_at: at,
    });
}

fn user_queries(text: &str) -> Vec<String> {
    let mut rest = text;
    let mut found = Vec::new();
    while let Some(start) = rest.find("<user_query>") {
        rest = &rest[start + "<user_query>".len()..];
        if let Some(end) = rest.find("</user_query>") {
            let query = rest[..end].trim();
            if !query.is_empty() {
                found.push(query.to_string());
            }
            rest = &rest[end + "</user_query>".len()..];
        } else {
            let query = rest.trim();
            if !query.is_empty() {
                found.push(query.to_string());
            }
            break;
        }
    }
    found
}

fn injected(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("<user_info>")
        || trimmed.starts_with("<system-reminder>")
        || trimmed.starts_with("<user_instructions>")
        || trimmed.starts_with("<environment_context>")
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

    fn fixture() -> GrokScanner {
        GrokScanner::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grok"))
    }

    fn by_id(sessions: &[SessionSummary], id: &str) -> SessionSummary {
        sessions
            .iter()
            .find(|session| session.session_id == id)
            .cloned()
            .unwrap_or_else(|| panic!("missing {id}"))
    }

    #[test]
    fn scans_summary_and_decoded_cwd() {
        let sessions = fixture().scan().unwrap();
        let main = by_id(&sessions, "33333333-3333-4333-8333-333333333333");
        assert_eq!(main.cwd, PathBuf::from("/tmp/grok-demo"));
        assert_eq!(main.title, "Fix the widget");
        assert_eq!(main.model.as_deref(), Some("grok-4.6"));
        assert!(main.resumable);
        assert_eq!(
            main.created_at,
            parse_rfc3339("2026-08-24T10:00:00.000000Z").unwrap()
        );

        let fallback = by_id(&sessions, "44444444-4444-4444-8444-444444444444");
        assert_eq!(fallback.cwd, PathBuf::from("/tmp/grok-fallback"));
        assert_eq!(fallback.title, "Please rename the button");
        assert!(!by_id(&sessions, "not-a-uuid").resumable);
        assert_eq!(
            by_id(&sessions, "not-a-uuid").cwd,
            PathBuf::from("/tmp/from-dirname")
        );
    }

    #[test]
    fn loads_updates_and_chat_history() {
        let scanner = fixture();
        let sessions = scanner.scan().unwrap();
        let main = scanner
            .load(&by_id(&sessions, "33333333-3333-4333-8333-333333333333"))
            .unwrap();
        let texts: Vec<_> = main
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "Fix the widget"),
                (Role::Assistant, "I updated the widget."),
                (Role::Assistant, "Done."),
            ]
        );
        assert_eq!(main.summary.message_count, Some(3));
        assert_eq!(main.messages[0].created_at, unix_to_utc(1_780_000_000));

        let fallback = scanner
            .load(&by_id(&sessions, "44444444-4444-4444-8444-444444444444"))
            .unwrap();
        let texts: Vec<_> = fallback
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "Please rename the button"),
                (Role::Assistant, "Renamed it.")
            ]
        );
    }

    #[test]
    fn missing_home_is_empty() {
        assert!(
            GrokScanner::new(PathBuf::from("/no/such/grok-home"))
                .scan()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn prefers_the_richer_transcript() {
        let scanner = fixture();
        let sessions = scanner.scan().unwrap();
        let truncated = scanner
            .load(&by_id(&sessions, "77777777-7777-4777-8777-777777777777"))
            .unwrap();
        let texts: Vec<_> = truncated
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "Where is the reply"),
                (Role::Assistant, "It is in the chat history.")
            ]
        );

        let tie = scanner
            .load(&by_id(&sessions, "88888888-8888-4888-8888-888888888888"))
            .unwrap();
        let texts: Vec<_> = tie
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(texts, vec!["same prompt", "from updates"]);
    }

    #[test]
    fn updates_read_error_uses_chat_history() {
        let reader = BufReader::new(ErrorRead);
        assert!(updates_from_reader(reader).is_err());

        let dir =
            std::env::temp_dir().join(format!("t3-import-grok-readerr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("updates.jsonl")).unwrap();
        std::fs::write(
            dir.join("chat_history.jsonl"),
            "{\"type\":\"user\",\"content\":\"<user_query>\\nfrom chat\\n</user_query>\"}\n{\"type\":\"assistant\",\"content\":\"chat reply\"}\n",
        )
        .unwrap();
        let transcript = load_dir(
            &dir,
            &SessionSummary {
                provider: Provider::Grok,
                session_id: "read-error".to_string(),
                cwd: PathBuf::new(),
                title: "Untitled".to_string(),
                created_at: DateTime::UNIX_EPOCH,
                updated_at: DateTime::UNIX_EPOCH,
                message_count: None,
                model: None,
                source: dir.clone(),
                resumable: false,
            },
        )
        .unwrap();
        let texts: Vec<_> = transcript
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(texts, vec!["from chat", "chat reply"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct ErrorRead;

    impl std::io::Read for ErrorRead {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("read failed"))
        }
    }
}
