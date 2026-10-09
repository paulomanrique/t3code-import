use std::path::{Path, PathBuf};

/// One canonical T3 Code home (`T3CODE_HOME`, default `~/.t3`). Every read, write
/// and `t3` CLI call targets this same directory.
#[derive(Debug, Clone)]
pub struct T3Home {
    pub base: PathBuf,
}

impl T3Home {
    pub fn detect(override_base: Option<PathBuf>) -> anyhow::Result<Self> {
        let base = override_base
            .or_else(|| std::env::var_os("T3CODE_HOME").map(PathBuf::from))
            .or_else(|| dirs::home_dir().map(|p| p.join(".t3")))
            .ok_or_else(|| anyhow::anyhow!("cannot determine T3 home"))?;
        anyhow::ensure!(
            base.join("userdata/statev2.sqlite").is_file(),
            "T3 database missing: {}",
            base.join("userdata/statev2.sqlite").display()
        );
        Ok(Self {
            base: base.canonicalize()?,
        })
    }

    pub fn userdata(&self) -> PathBuf {
        self.base.join("userdata")
    }

    pub fn db_path(&self) -> PathBuf {
        self.userdata().join("statev2.sqlite")
    }

    /// The `t3` CLI that belongs to this home, if installed.
    pub fn cli(&self) -> Option<PathBuf> {
        let local = self.base.join("bin/t3");
        if executable(&local) {
            return Some(local);
        }
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join(if cfg!(windows) { "t3.exe" } else { "t3" }))
                .find(|p| executable(p))
        })
    }

    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.base)
    }
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
