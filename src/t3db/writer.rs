use super::{ImportOptions, ImportReport, ImportRequest, Progress, Snapshot, T3Home};

pub fn snapshot(home: &T3Home) -> anyhow::Result<Snapshot> {
    todo!("read-only snapshot of {home:?}")
}

pub fn import(
    home: &T3Home,
    requests: Vec<ImportRequest>,
    options: &ImportOptions,
    progress: &mut dyn FnMut(Progress),
) -> anyhow::Result<ImportReport> {
    let _ = (requests, options, progress);
    todo!("guarded import into {home:?}")
}
