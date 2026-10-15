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
    /// The T3 CLI used to create projects. T3's own `bin/t3` wins; otherwise the
    /// newest `t3` on PATH. Not simply the first one: an old global install can
    /// shadow the current one, and an old CLI writes projects to a database the
    /// running T3 no longer reads.
    pub fn cli(&self) -> Option<PathBuf> {
        let local = self.base.join("bin/t3");
        if executable(&local) {
            return Some(local);
        }
        let names: &[&str] = if cfg!(windows) { &["t3.exe", "t3.cmd"] } else { &["t3"] };
        let mut seen = std::collections::HashSet::new();
        let candidates: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths)
                    .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
                    .filter(|p| executable(p))
                    .filter(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())))
                    .collect()
            })
            .unwrap_or_default();
        if candidates.len() <= 1 {
            return candidates.into_iter().next();
        }
        // Ties keep PATH order: max_by_key returns the last maximum, so go in reverse.
        candidates.into_iter().rev().max_by_key(|p| cli_version(p))
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

/// `t3 --version`, as a sortable key. Unreadable versions sort first.
fn cli_version(cli: &Path) -> Option<CliVersion> {
    let mut cmd = std::process::Command::new(cli);
    cmd.arg("--version");
    let out = super::control::output_timeout(&mut cmd, std::time::Duration::from_secs(10)).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_cli_version(&String::from_utf8_lossy(&out.stdout))
}

/// major, minor, patch, "is a release", then the prerelease parts.
type CliVersion = (u64, u64, u64, bool, Vec<PrePart>);

/// A prerelease part: numbers compare as numbers, so build 1000 is after 999.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PrePart {
    Num(u64),
    Text(String),
}

/// "t3 v0.0.45" or "t3 v0.0.31-nightly.20260729.946". A release outranks the
/// nightlies leading up to it; nightlies of one version sort by date, then build.
fn parse_cli_version(text: &str) -> Option<CliVersion> {
    let v = text.split_whitespace().find_map(|w| w.strip_prefix('v'))?;
    let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
    let mut nums = core.split('.').map(|n| n.parse::<u64>().ok());
    let (major, minor, patch) = (nums.next()??, nums.next()??, nums.next()??);
    let parts = pre
        .unwrap_or("")
        .split('.')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().map_or_else(|_| PrePart::Text(p.to_string()), PrePart::Num))
        .collect();
    Some((major, minor, patch, pre.is_none(), parts))
}

#[cfg(test)]
mod tests {
    use super::parse_cli_version;

    #[test]
    fn newest_cli_version_wins() {
        let old = parse_cli_version("t3 v0.0.31-nightly.20260729.946\n");
        let release = parse_cli_version("t3 v0.0.45\n");
        let nightly = parse_cli_version("t3 v0.0.46-nightly.20261014.3118");
        assert!(old < release && release < nightly);
        assert!(parse_cli_version("t3 v0.0.45-nightly.20261001.1") < release);
        assert!(
            parse_cli_version("t3 v0.0.46-nightly.20261014.999") < parse_cli_version("t3 v0.0.46-nightly.20261014.1000")
        );
        assert_eq!(parse_cli_version("garbage"), None);
    }
}
