use std::path::{Path, PathBuf};

/// Where an imported thread lives in T3: the project root, plus the worktree it
/// originally ran in when that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub workspace_root: PathBuf,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    /// The worktree is gone; resuming will run in `workspace_root`.
    pub worktree_missing: bool,
}

/// Maps a session cwd to its project. A git linked worktree maps to its main
/// repository with `worktree_path` kept; anything else is its own root.
pub fn resolve_placement(cwd: &Path) -> Placement {
    todo!("git worktree detection for {cwd:?}")
}
