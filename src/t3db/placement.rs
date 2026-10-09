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
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut placement = Placement {
        workspace_root: cwd.clone(),
        worktree_path: None,
        branch: None,
        worktree_missing: !cwd.exists(),
    };
    for root in cwd.ancestors() {
        let marker = root.join(".git");
        let gitdir = if marker.is_dir() {
            marker.clone()
        } else {
            let Ok(text) = std::fs::read_to_string(&marker) else {
                continue;
            };
            let Some(path) = text.trim().strip_prefix("gitdir:") else {
                continue;
            };
            root.join(path.trim())
        };
        placement.branch = std::fs::read_to_string(gitdir.join("HEAD"))
            .ok()
            .and_then(|s| s.trim().strip_prefix("ref: refs/heads/").map(str::to_owned));
        if let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) {
            if let Ok(common) = gitdir.join(common.trim()).canonicalize() {
                // A standard linked worktree shares the main repository's .git directory.
                if common.file_name().is_some_and(|p| p == ".git") {
                    if let Some(main) = common.parent() {
                        placement.workspace_root = main.to_path_buf();
                        placement.worktree_path = Some(root.to_path_buf());
                    }
                }
            }
        }
        break;
    }
    placement
}
