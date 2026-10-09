use std::path::{Path, PathBuf};

/// One canonical T3 Code home (`T3CODE_HOME`, default `~/.t3`). Every read, write
/// and `t3` CLI call targets this same directory.
#[derive(Debug, Clone)]
pub struct T3Home {
    pub base: PathBuf,
}

impl T3Home {
    pub fn detect(override_base: Option<PathBuf>) -> anyhow::Result<Self> {
        todo!("resolve override > T3CODE_HOME > ~/.t3 and verify userdata/statev2.sqlite exists: {override_base:?}")
    }

    pub fn userdata(&self) -> PathBuf {
        self.base.join("userdata")
    }

    pub fn db_path(&self) -> PathBuf {
        self.userdata().join("statev2.sqlite")
    }

    /// The `t3` CLI that belongs to this home, if installed.
    pub fn cli(&self) -> Option<PathBuf> {
        todo!("~/.t3/bin/t3 then PATH")
    }

    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.base)
    }
}
