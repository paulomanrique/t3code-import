use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde_json::Value;

use crate::model::{Message, Provider, Role, SessionSummary, Transcript};

use super::Scanner;
use super::util::{
    extract_text, file_mtime, json_string_first, parse_rfc3339, plausible_model, title_from_text,
};

pub struct CodexScanner {
    root: PathBuf,
}

impl CodexScanner {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Scanner for CodexScanner {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn scan(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let mut files = Vec::new();
        collect_rollouts(&self.root.join("sessions"), &mut files);
        collect_rollouts(&self.root.join("archived_sessions"), &mut files);
        let mut sessions: Vec<SessionSummary> = files
            .par_iter()
            .filter_map(|path| scan_file(path))
            .collect();
        dedupe_newer(&mut sessions);
        Ok(sessions)
    }

    fn load(&self, summary: &SessionSummary) -> anyhow::Result<Transcript> {
        load_file(&summary.source, summary)
            .with_context(|| format!("load codex session {}", summary.session_id))
    }
}

fn collect_rollouts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_rollouts(&path, out);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("rollout-") && name.ends_with(".jsonl") {
            out.push(path);
        }
    }
}

#[derive(Clone)]
enum Rec {
    Skip,
    Meta {
        id: String,
        cwd: Option<String>,
        at: Option<DateTime<Utc>>,
    },
    Turn {
        model: Option<String>,
        cwd: Option<String>,
        at: Option<DateTime<Utc>>,
    },
    EventUser {
        text: String,
        at: Option<DateTime<Utc>>,
    },
    ResponseUser {
        text: String,
        turn_id: Option<String>,
        at: Option<DateTime<Utc>>,
    },
    Assistant {
        text: String,
        at: Option<DateTime<Utc>>,
    },
}

impl Rec {
    fn at(&self) -> Option<DateTime<Utc>> {
        match self {
            Rec::Meta { at, .. }
            | Rec::Turn { at, .. }
            | Rec::EventUser { at, .. }
            | Rec::ResponseUser { at, .. }
            | Rec::Assistant { at, .. } => *at,
            Rec::Skip => None,
        }
    }
}

fn scan_file(path: &Path) -> Option<SessionSummary> {
    let file = File::open(path).ok()?;
    let mtime = file_mtime(path);
    let mut reader = BufReader::with_capacity(128 * 1024, file);
    let mut records = Vec::new();
    let mut saw_user = false;
    let mut raw = String::new();
    loop {
        raw.clear();
        let read = match reader.read_line(&mut raw) {
            Ok(read) => read,
            Err(_) => continue,
        };
        if read == 0 {
            break;
        }
        if raw.ends_with('\n') {
            raw.pop();
        }
        if raw.ends_with('\r') {
            raw.pop();
        }
        if raw.is_empty() {
            continue;
        }
        if !should_parse(&raw) {
            records.push(Rec::Skip);
            continue;
        }
        let rec = parse_line(&raw);
        let userish = matches!(rec, Rec::EventUser { .. } | Rec::ResponseUser { .. });
        let assistant = matches!(rec, Rec::Assistant { .. });
        if userish {
            saw_user = true;
        }
        records.push(rec);
        // The first assistant closes the opening turn, which is enough for the title.
        if assistant && saw_user {
            break;
        }
    }
    let parsed = project(&records, mtime);
    let id = parsed.id?;
    let tail_model = last_turn_model(path);
    let first_user = parsed
        .messages
        .iter()
        .find(|message| message.role == Role::User)
        .map(|message| message.text.clone());
    Some(SessionSummary {
        provider: Provider::Codex,
        session_id: id,
        cwd: parsed.cwd.unwrap_or_default(),
        title: first_user
            .as_deref()
            .map(title_from_text)
            .unwrap_or_else(|| "Untitled".to_string()),
        created_at: parsed.created.unwrap_or(mtime),
        updated_at: mtime,
        message_count: None,
        model: tail_model.or(parsed.model),
        source: path.to_path_buf(),
        resumable: true,
    })
}

fn load_file(path: &Path, summary: &SessionSummary) -> anyhow::Result<Transcript> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mtime = file_mtime(path);
    let reader = BufReader::with_capacity(128 * 1024, file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        if !should_parse(&line) {
            records.push(Rec::Skip);
            continue;
        }
        records.push(parse_line(&line));
    }
    let parsed = project(&records, mtime);
    let id = parsed
        .id
        .ok_or_else(|| anyhow::anyhow!("codex transcript {} has no session id", path.display()))?;
    let mut summary = summary.clone();
    summary.session_id = id;
    if let Some(cwd) = parsed.cwd {
        summary.cwd = cwd;
    }
    summary.model = parsed.model.or(summary.model);
    summary.created_at = parsed.created.unwrap_or(summary.created_at);
    summary.updated_at = mtime;
    summary.title = parsed
        .messages
        .iter()
        .find(|message| message.role == Role::User)
        .map(|message| title_from_text(&message.text))
        .unwrap_or_else(|| "Untitled".to_string());
    summary.message_count = Some(parsed.messages.len());
    Ok(Transcript {
        summary,
        messages: parsed.messages,
    })
}

fn should_parse(line: &str) -> bool {
    if line.len() < 256 * 1024 {
        return true;
    }
    line.contains("\"session_meta\"")
        || line.contains("\"turn_context\"")
        || line.contains("\"user_message\"")
        || line.contains("\"role\":\"user\"")
        || line.contains("\"role\":\"assistant\"")
        || line.contains("\"role\": \"user\"")
        || line.contains("\"role\": \"assistant\"")
}

fn parse_line(line: &str) -> Rec {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Rec::Skip;
    };
    let at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339);
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = value.get("payload");
    match kind {
        "session_meta" => {
            let id = payload
                .and_then(|payload| payload.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .or_else(|| {
                    payload
                        .and_then(|payload| payload.get("session_id"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                });
            let Some(id) = id else { return Rec::Skip };
            Rec::Meta {
                id: id.to_string(),
                cwd: cwd_of(payload, &value),
                at,
            }
        }
        "turn_context" => {
            let model = payload
                .and_then(|payload| payload.get("model"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|model| plausible_model(model))
                .map(str::to_string);
            Rec::Turn {
                model,
                cwd: cwd_of(payload, &value),
                at,
            }
        }
        "event_msg" => {
            let Some(payload) = payload else {
                return Rec::Skip;
            };
            if payload.get("type").and_then(Value::as_str) != Some("user_message") {
                return Rec::Skip;
            }
            let text = payload.get("message").and_then(Value::as_str).unwrap_or("");
            if text.trim().is_empty() {
                return Rec::Skip;
            }
            Rec::EventUser {
                text: text.to_string(),
                at,
            }
        }
        "response_item" => {
            let Some(payload) = payload else {
                return Rec::Skip;
            };
            if payload.get("type").and_then(Value::as_str) != Some("message") {
                return Rec::Skip;
            }
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            let text = payload.get("content").map(extract_text).unwrap_or_default();
            if text.is_empty() {
                return Rec::Skip;
            }
            match role {
                "assistant" => Rec::Assistant { text, at },
                "user" if is_injected_context(&text) => Rec::Skip,
                "user" => Rec::ResponseUser {
                    text,
                    turn_id: turn_id(payload),
                    at,
                },
                _ => Rec::Skip,
            }
        }
        _ => Rec::Skip,
    }
}

/// Setup text Codex sends as a user message: project instructions, the
/// environment block, plugin hints, abort notices. Interactive sessions also mark
/// the real prompt with an `event_msg`, but `codex exec` rollouts don't, so match
/// the wrappers: AGENTS.md, or a message that opens with a bare `<tag>` line.
fn is_injected_context(text: &str) -> bool {
    let text = text.trim_start();
    if text.starts_with("# AGENTS.md instructions") {
        return true;
    }
    let first = text.lines().next().unwrap_or("").trim();
    first.len() > 2
        && first.starts_with('<')
        && first.ends_with('>')
        && first[1..first.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' '))
}

fn cwd_of(payload: Option<&Value>, record: &Value) -> Option<String> {
    payload
        .and_then(|payload| payload.get("cwd"))
        .and_then(Value::as_str)
        .or_else(|| record.get("cwd").and_then(Value::as_str))
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_string)
}

fn turn_id(payload: &Value) -> Option<String> {
    let id = payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|meta| meta.get("turn_id"))
        .and_then(Value::as_str)?
        .trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

struct Projected {
    id: Option<String>,
    cwd: Option<PathBuf>,
    model: Option<String>,
    created: Option<DateTime<Utc>>,
    messages: Vec<Message>,
}

fn project(records: &[Rec], fallback: DateTime<Utc>) -> Projected {
    let mut id = None;
    let mut cwd = None;
    let mut model = None;
    // A response-item user is dropped for the whole turn when that turn's id also
    // produced an event_msg whose text matches one of the response copies. That is
    // how Codex separates injected setup text from the prompt the user submitted.
    let mut canonical = std::collections::HashSet::new();
    let mut texts_in_turn = std::collections::HashSet::new();
    let mut users_in_turn: Vec<(usize, String, String)> = Vec::new();
    let finish = |canonical: &mut std::collections::HashSet<usize>,
                  texts: &mut std::collections::HashSet<String>,
                  users: &mut Vec<(usize, String, String)>| {
        let turn_ids: std::collections::HashSet<String> = users
            .iter()
            .filter(|(_, _, text)| texts.contains(text))
            .map(|(_, turn, _)| turn.clone())
            .collect();
        for (index, turn, _) in users.iter() {
            if turn_ids.contains(turn) {
                canonical.insert(*index);
            }
        }
        texts.clear();
        users.clear();
    };
    for (index, record) in records.iter().enumerate() {
        match record {
            Rec::Assistant { .. } => finish(&mut canonical, &mut texts_in_turn, &mut users_in_turn),
            Rec::EventUser { text, .. } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    texts_in_turn.insert(trimmed.to_string());
                }
            }
            Rec::ResponseUser {
                text,
                turn_id: Some(turn),
                ..
            } if !text.is_empty() => {
                users_in_turn.push((index, turn.clone(), text.clone()));
            }
            _ => {}
        }
    }
    finish(&mut canonical, &mut texts_in_turn, &mut users_in_turn);

    for record in records {
        match record {
            Rec::Meta {
                id: session_id,
                cwd: meta_cwd,
                ..
            } => {
                if id.is_none() {
                    id = Some(session_id.clone());
                }
                if cwd.is_none() {
                    if let Some(value) = meta_cwd {
                        cwd = Some(PathBuf::from(value));
                    }
                }
            }
            Rec::Turn {
                model: turn_model,
                cwd: turn_cwd,
                ..
            } => {
                if let Some(value) = turn_model {
                    model = Some(value.clone());
                }
                if cwd.is_none() {
                    if let Some(value) = turn_cwd {
                        cwd = Some(PathBuf::from(value));
                    }
                }
            }
            _ => {}
        }
    }

    let messages = assemble(records, &canonical, fallback);
    let created = records.iter().find_map(Rec::at);
    Projected {
        id,
        cwd,
        model,
        created,
        messages,
    }
}

fn matching_event(built: &[BuiltMsg], text: &str) -> bool {
    let comparison = text.trim();
    for message in built.iter().rev() {
        if message.role == Role::Assistant {
            return false;
        }
        if message.role == Role::User && !message.response_user && message.text.trim() == comparison
        {
            return true;
        }
    }
    false
}

struct BuiltMsg {
    role: Role,
    text: String,
    at: DateTime<Utc>,
    response_user: bool,
}

fn assemble(
    records: &[Rec],
    canonical: &std::collections::HashSet<usize>,
    fallback: DateTime<Utc>,
) -> Vec<Message> {
    let mut built: Vec<BuiltMsg> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        match record {
            Rec::EventUser { text, at } => {
                if text.trim().is_empty() {
                    continue;
                }
                let trimmed = text.trim();
                if let Some(position) =
                    built
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(position, message)| {
                            if message.role == Role::Assistant {
                                return Some(None);
                            }
                            if message.response_user && message.text.trim() == trimmed {
                                Some(Some(position))
                            } else {
                                None
                            }
                        })
                {
                    if let Some(position) = position {
                        built.remove(position);
                    }
                }
                built.push(BuiltMsg {
                    role: Role::User,
                    text: text.clone(),
                    at: at.unwrap_or(fallback),
                    response_user: false,
                });
            }
            Rec::ResponseUser { text, at, .. } => {
                if text.is_empty() || canonical.contains(&index) || matching_event(&built, text) {
                    continue;
                }
                built.push(BuiltMsg {
                    role: Role::User,
                    text: text.clone(),
                    at: at.unwrap_or(fallback),
                    response_user: true,
                });
            }
            Rec::Assistant { text, at } => {
                if text.is_empty() {
                    continue;
                }
                built.push(BuiltMsg {
                    role: Role::Assistant,
                    text: text.clone(),
                    at: at.unwrap_or(fallback),
                    response_user: false,
                });
            }
            _ => {}
        }
    }
    built
        .into_iter()
        .map(|message| Message {
            role: message.role,
            text: message.text,
            created_at: message.at,
        })
        .collect()
}

fn last_turn_model(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(512 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    // The seek can land mid-line (and mid-character). Drop that partial record.
    if start > 0 {
        if let Some(pos) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=pos);
        }
    }
    let tail = String::from_utf8_lossy(&bytes);
    let mut last = None;
    let mut from = 0;
    while let Some(relative) = tail[from..].find("turn_context") {
        let at = from + relative;
        let end = char_end(&tail, at.saturating_add(8_000));
        let window = &tail[at..end];
        if let Some(model) = json_string_first(window, "model") {
            if plausible_model(&model) {
                last = Some(model);
            }
        }
        from = at + "turn_context".len();
    }
    last
}

fn char_end(text: &str, mut index: usize) -> usize {
    if index > text.len() {
        index = text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
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

    fn fixture() -> CodexScanner {
        CodexScanner::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex"))
    }

    fn by_id(sessions: &[SessionSummary], id: &str) -> SessionSummary {
        sessions
            .iter()
            .find(|session| session.session_id == id)
            .cloned()
            .unwrap_or_else(|| panic!("missing {id}"))
    }

    #[test]
    fn scans_live_and_archived_sessions() {
        let sessions = fixture().scan().unwrap();
        let ids: Vec<_> = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert!(ids.contains(&"22222222-2222-4222-8222-222222222222"));
        assert!(ids.contains(&"55555555-5555-4555-8555-555555555555"));
        assert!(ids.contains(&"66666666-6666-4666-8666-666666666666"));
        assert!(!ids.iter().any(|id| id.contains("not-a-session")));

        let main = by_id(&sessions, "22222222-2222-4222-8222-222222222222");
        assert_eq!(main.cwd, PathBuf::from("/tmp/codex-demo"));
        assert_eq!(main.title, "Fix the actual bug");
        assert_eq!(main.model.as_deref(), Some("gpt-later"));
        assert!(main.resumable);
        assert!(main.message_count.is_none());

        let archived = by_id(&sessions, "55555555-5555-4555-8555-555555555555");
        assert_eq!(archived.cwd, PathBuf::from("/tmp/archived"));
        assert_eq!(archived.title, "Initialize Git and add a README.");

        let fork = by_id(&sessions, "66666666-6666-4666-8666-666666666666");
        assert_eq!(fork.cwd, PathBuf::from("/tmp/fork"));
        assert_eq!(fork.title, "Continue in the fork");
    }

    #[test]
    fn loads_deduped_turns() {
        let scanner = fixture();
        let sessions = scanner.scan().unwrap();
        let main = scanner
            .load(&by_id(&sessions, "22222222-2222-4222-8222-222222222222"))
            .unwrap();
        let texts: Vec<_> = main
            .messages
            .iter()
            .map(|message| (message.role, message.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Role::User, "Fix the actual bug"),
                (Role::Assistant, "Fixed"),
                (Role::User, "Second prompt"),
                (Role::Assistant, "Second reply"),
            ]
        );
        assert_eq!(main.summary.model.as_deref(), Some("gpt-later"));
        assert_eq!(main.summary.message_count, Some(4));

        let archived = scanner
            .load(&by_id(&sessions, "55555555-5555-4555-8555-555555555555"))
            .unwrap();
        let texts: Vec<_> = archived
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec!["Initialize Git and add a README."]
        );
    }

    #[test]
    fn missing_home_is_empty() {
        assert!(
            CodexScanner::new(PathBuf::from("/no/such/codex-home"))
                .scan()
                .unwrap()
                .is_empty()
        );
    }
}
