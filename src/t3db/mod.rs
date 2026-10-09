//! Everything that reads or writes T3 Code's `statev2.sqlite`.
//!
//! Writes replicate exactly what `apps/server/src/project/AgentSessionImporter.ts`
//! produces through `EventSink.write`, minus its limits. Nothing is written unless
//! [`guard`] accepts the database first.

pub mod guard;
pub mod home;
pub mod placement;
pub mod writer;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::model::{Provider, Transcript};

pub use guard::{GuardFinding, GuardReport};
pub use home::T3Home;
pub use placement::{Placement, resolve_placement};

#[derive(Debug, Clone)]
pub struct ProjectRef {
    pub id: String,
    pub title: String,
    pub workspace_root: PathBuf,
}

/// Read-only view of the T3 state the picker needs. Safe while T3 is running.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub projects: Vec<ProjectRef>,
    /// Ids of every V2 thread already in T3 (`orchestration_v2_projection_threads`).
    pub thread_ids: HashSet<String>,
    /// Enabled provider instance ids per driver, from T3's `settings.json`.
    /// The first entry is the default for that driver.
    pub provider_instances: HashMap<Provider, Vec<String>>,
}

pub fn snapshot(home: &T3Home) -> anyhow::Result<Snapshot> {
    writer::snapshot(home)
}

/// Deterministic app-thread id, identical to the native importer's.
pub fn thread_id_for(provider_instance_id: &str, session_id: &str) -> String {
    format!("import:{provider_instance_id}:{session_id}")
}

#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub transcript: Transcript,
    pub provider_instance_id: String,
    pub placement: Placement,
}

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Keep only the newest N messages. `None` imports everything.
    pub max_messages: Option<usize>,
    /// Run the guard and plan the import without creating projects or writing.
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub imported: Vec<String>,
    /// (thread id, reason)
    pub skipped: Vec<(String, String)>,
    pub projects_created: Vec<PathBuf>,
    pub backup: Option<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Progress {
    Guard,
    Backup,
    CreatingProject(PathBuf),
    Writing { done: usize, total: usize },
    Verifying,
}

/// Checks that T3 is closed and that the tables and stored payloads match what this
/// build knows how to write.
pub fn check(home: &T3Home) -> anyhow::Result<GuardReport> {
    guard::check(home)
}

pub fn import(
    home: &T3Home,
    requests: Vec<ImportRequest>,
    options: &ImportOptions,
    progress: &mut dyn FnMut(Progress),
) -> anyhow::Result<ImportReport> {
    writer::import(home, requests, options, progress)
}
