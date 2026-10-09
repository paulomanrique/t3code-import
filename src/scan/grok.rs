use std::path::PathBuf;

use crate::model::{Provider, SessionSummary, Transcript};

use super::Scanner;

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
        let _ = &self.root;
        todo!("scan grok sessions")
    }

    fn load(&self, _summary: &SessionSummary) -> anyhow::Result<Transcript> {
        todo!("load grok transcript")
    }
}
