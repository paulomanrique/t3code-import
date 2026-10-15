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
    if trimmed.is_empty() || is_placeholder_title(trimmed) {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// First line of a real user message, trimmed to [`TITLE_CHARS`] characters.
/// Attached-image markers (`<image name=… path=…>`, `</image>`) are skipped, so a
/// prompt that starts with a pasted screenshot is titled by its text.
pub(super) fn title_from_text(text: &str) -> String {
    let line = text
        .lines()
        .map(strip_image_tags)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let trimmed: String = line.chars().take(TITLE_CHARS).collect();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Removes complete `<image …>` and `</image>` markers. Other text that merely
/// starts with "<image" (`<images>`, `<imageProcessing>`, a stray `<image`) stays.
fn strip_image_tags(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('<') {
        let tail = &rest[start..];
        let marker_len = if tail.starts_with("</image>") {
            Some("</image>".len())
        } else if tail.starts_with("<image")
            && tail[6..].starts_with(|c: char| c == '>' || c.is_whitespace())
        {
            tail.find('>').map(|end| end + 1)
        } else {
            None
        };
        match marker_len {
            Some(len) => {
                out.push_str(&rest[..start]);
                rest = &rest[start + len..];
            }
            None => {
                out.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

pub(super) fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(super) fn unix_to_utc(raw: i64) -> DateTime<Utc> {
    // `i64::abs` panics on `i64::MIN` when overflow checks are on.
    let magnitude = raw.unsigned_abs();
    let (secs, nanos) = if magnitude > 10_000_000_000_000 {
        (
            raw.div_euclid(1_000_000),
            raw.rem_euclid(1_000_000) as u32 * 1_000,
        )
    } else if magnitude > 10_000_000_000 {
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

/// Titles a CLI assigns before anything is said, like OpenCode's
/// `New session - 2026-08-16T07:01:52.600Z`. The first prompt names it better.
fn is_placeholder_title(title: &str) -> bool {
    title
        .strip_prefix("New session - ")
        .is_some_and(|rest| rest.len() >= 10 && rest.as_bytes()[..4].iter().all(u8::is_ascii_digit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_to_utc_handles_i64_min() {
        let secs = i64::MIN.div_euclid(1_000_000);
        let nanos = i64::MIN.rem_euclid(1_000_000) as u32 * 1_000;
        assert_eq!(
            unix_to_utc(i64::MIN),
            DateTime::from_timestamp(secs, nanos).unwrap_or(DateTime::UNIX_EPOCH)
        );
        assert_eq!(
            unix_to_utc(1_700_000_000),
            DateTime::from_timestamp(1_700_000_000, 0).unwrap()
        );
        assert_eq!(
            unix_to_utc(1_700_000_000_123),
            DateTime::from_timestamp(1_700_000_000, 123_000_000).unwrap()
        );
        assert_eq!(
            unix_to_utc(1_700_000_000_123_456),
            DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap()
        );
    }

    #[test]
    fn title_skips_attached_image_markers() {
        let text = "<image name=[Image #1] path=\"/var/folders/x/shot.png\">\n</image>\nWhy is this button red?";
        assert_eq!(title_from_text(text), "Why is this button red?");
        assert_eq!(title_from_text("see <image name=[Image #2]> here"), "see  here");
        assert_eq!(title_from_text("<image name=[Image #1]>"), "Untitled");
        assert_eq!(title_from_text("<images> are listed"), "<images> are listed");
        assert_eq!(title_from_text("fix <imageProcessing> lag"), "fix <imageProcessing> lag");
        assert_eq!(title_from_text("broken <image name=x"), "broken <image name=x");
    }
}
