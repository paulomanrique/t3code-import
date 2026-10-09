//! One scanner per agent CLI. Scanners never write and never apply limits.

mod claude;
mod codex;
mod grok;
mod opencode;
mod util;

use std::path::PathBuf;

use crate::model::{Provider, SessionSummary, Transcript};

pub trait Scanner: Send + Sync {
    fn provider(&self) -> Provider;
    /// Lists every session found. Unreadable entries are skipped, not fatal.
    fn scan(&self) -> anyhow::Result<Vec<SessionSummary>>;
    /// Loads the complete text history of one session returned by [`Scanner::scan`].
    fn load(&self, summary: &SessionSummary) -> anyhow::Result<Transcript>;
}

/// Where each CLI keeps its sessions. Defaults honor the CLIs' own env overrides.
#[derive(Debug, Clone)]
pub struct Roots {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub grok: PathBuf,
    pub opencode: PathBuf,
}

impl Roots {
    pub fn detect() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        Roots {
            claude: env("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            codex: env("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            grok: env("GROK_HOME").unwrap_or_else(|| home.join(".grok")),
            opencode: env("XDG_DATA_HOME")
                .map(|d| d.join("opencode"))
                .unwrap_or_else(|| home.join(".local/share/opencode")),
        }
    }
}

pub fn scanners(roots: &Roots) -> Vec<Box<dyn Scanner>> {
    vec![
        Box::new(claude::ClaudeScanner::new(roots.claude.clone())),
        Box::new(codex::CodexScanner::new(roots.codex.clone())),
        Box::new(grok::GrokScanner::new(roots.grok.clone())),
        Box::new(opencode::OpenCodeScanner::new(roots.opencode.clone())),
    ]
}

pub fn scanner_for(roots: &Roots, provider: Provider) -> Box<dyn Scanner> {
    scanners(roots)
        .into_iter()
        .find(|s| s.provider() == provider)
        .expect("every provider has a scanner")
}
