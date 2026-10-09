use std::path::PathBuf;

use crate::model::{Provider, SessionSummary, Transcript};

use super::Scanner;

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
        let _ = &self.root;
        todo!("scan claude sessions")
    }

    fn load(&self, _summary: &SessionSummary) -> anyhow::Result<Transcript> {
        todo!("load claude transcript")
    }
}
