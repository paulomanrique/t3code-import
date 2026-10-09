use std::path::PathBuf;

use crate::model::{Provider, SessionSummary, Transcript};

use super::Scanner;

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
        let _ = &self.root;
        todo!("scan codex sessions")
    }

    fn load(&self, _summary: &SessionSummary) -> anyhow::Result<Transcript> {
        todo!("load codex transcript")
    }
}
