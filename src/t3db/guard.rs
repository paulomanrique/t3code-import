use super::T3Home;

#[derive(Debug, Clone)]
pub struct GuardFinding {
    /// Table, column or payload field that differs.
    pub location: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Default)]
pub struct GuardReport {
    /// T3 server/app/service processes for this home that are still alive.
    pub t3_running: Vec<String>,
    pub findings: Vec<GuardFinding>,
    /// Rows sampled per table for the payload-shape check.
    pub sampled: Vec<(String, usize)>,
    pub warnings: Vec<String>,
    /// Prefilled GitHub "new issue" URL when there are findings.
    pub issue_url: Option<String>,
}

impl GuardReport {
    pub fn ok(&self) -> bool {
        self.t3_running.is_empty() && self.findings.is_empty()
    }
}

pub fn check(home: &T3Home) -> anyhow::Result<GuardReport> {
    todo!("structure + payload-shape guard for {home:?}")
}
