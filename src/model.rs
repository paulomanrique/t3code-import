//! Provider-neutral session model shared by the scanners, the TUI and the writer.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Grok,
    OpenCode,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::Claude, Provider::Codex, Provider::Grok, Provider::OpenCode];

    /// T3 Code driver kind (`ProviderDriverKind`).
    pub fn driver(self) -> &'static str {
        match self {
            Provider::Claude => "claudeAgent",
            Provider::Codex => "codex",
            Provider::Grok => "grok",
            Provider::OpenCode => "opencode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
            Provider::Grok => "Grok",
            Provider::OpenCode => "OpenCode",
        }
    }
}

/// Cheap metadata for one CLI session, produced by a scan. The full transcript is
/// loaded on demand with [`crate::scan::Scanner::load`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub provider: Provider,
    /// Native session id, exactly what the provider accepts to resume.
    pub session_id: String,
    /// Directory the session ran in.
    pub cwd: PathBuf,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// User + assistant text messages, when cheap to know at scan time.
    pub message_count: Option<usize>,
    /// Last model the session used, if recorded.
    pub model: Option<String>,
    /// File or database the session was read from.
    pub source: PathBuf,
    /// False when the provider cannot resume this session (e.g. non-UUID Claude ids).
    pub resumable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub summary: SessionSummary,
    /// Chronological user/assistant text, no tool calls.
    pub messages: Vec<Message>,
}
