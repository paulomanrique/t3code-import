//! Private helpers shared by the scanners. No I/O policy lives here.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde_json::Value;

/// Fallback title length for a first user message. Recorded titles are kept whole.
pub(super) const TITLE_CHARS: usize = 80;

pub(super) fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let groups = [8usize, 4, 4, 4, 12];
    let mut index = 0;
    for (group_index, group) in groups.iter().enumerate() {
        if group_index > 0 {
            if bytes.get(index) != Some(&b'-') {
                return false;
            }
            index += 1;
        }
        for _ in 0..*group {
            match bytes.get(index).map(|c| c.is_ascii_hexdigit()) {
                Some(true) => index += 1,
                _ => return false,
            }
        }
    }
    index == bytes.len()
}

pub(super) fn clean_recorded_title(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// First line of a real user message, trimmed to [`TITLE_CHARS`] characters.
pub(super) fn title_from_text(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let trimmed: String = line.chars().take(TITLE_CHARS).collect();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(super) fn unix_to_utc(raw: i64) -> DateTime<Utc> {
    let (secs, nanos) = if raw.abs() > 10_000_000_000_000 {
        (
            raw.div_euclid(1_000_000),
            raw.rem_euclid(1_000_000) as u32 * 1_000,
        )
    } else if raw.abs() > 10_000_000_000 {
        (
            raw.div_euclid(1_000),
            raw.rem_euclid(1_000) as u32 * 1_000_000,
        )
    } else {
        (raw, 0)
    };
    DateTime::from_timestamp(secs, nanos).unwrap_or(DateTime::UNIX_EPOCH)
}

pub(super) fn system_time_to_utc(time: SystemTime) -> DateTime<Utc> {
    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return DateTime::UNIX_EPOCH;
    };
    DateTime::from_timestamp(duration.as_secs() as i64, duration.subsec_nanos())
        .unwrap_or(DateTime::UNIX_EPOCH)
}

pub(super) fn file_mtime(path: &Path) -> DateTime<Utc> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(system_time_to_utc)
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// Text blocks the CLIs actually show. Tool calls, results and thinking are dropped.
pub(super) fn extract_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.trim().to_string(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                let kind = block.get("type").and_then(Value::as_str)?;
                if kind != "text" && kind != "input_text" && kind != "output_text" {
                    return None;
                }
                let text = block.get("text").and_then(Value::as_str)?.trim();
                if text.is_empty() {
                    None
                } else {
                    Some(text.to_string())
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn json_string_at(hay: &str, key: &str, last: bool) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut search_from = 0;
    let mut found = None;
    while search_from < hay.len() {
        let Some(relative) = hay[search_from..].find(&needle) else {
            break;
        };
        let after_key = search_from + relative + needle.len();
        let rest = hay[after_key..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if let Some(value) = parse_json_string(rest) {
                if !last {
                    return Some(value);
                }
                found = Some(value);
            }
        }
        search_from = after_key.max(search_from + 1);
    }
    found
}

pub(super) fn json_string_first(hay: &str, key: &str) -> Option<String> {
    json_string_at(hay, key, false)
}

pub(super) fn json_string_last(hay: &str, key: &str) -> Option<String> {
    json_string_at(hay, key, true)
}

fn parse_json_string(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return serde_json::from_str(&source[..=index]).ok(),
            _ => index += 1,
        }
    }
    None
}

pub(super) fn plausible_model(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value != "<synthetic>"
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
}
