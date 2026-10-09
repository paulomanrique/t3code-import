use std::path::PathBuf;

use crate::model::{Provider, SessionSummary, Transcript};

use super::Scanner;

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
        let _ = &self.root;
        todo!("scan opencode sessions")
    }

    fn load(&self, _summary: &SessionSummary) -> anyhow::Result<Transcript> {
        todo!("load opencode transcript")
    }
}
