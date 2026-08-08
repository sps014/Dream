//! `dream.lock` lockfile: the exact, checksum-pinned dependency graph resolved from `dream.toml`.
//! Checked into version control (like `Cargo.lock`) so every install is reproducible. Package
//! entries are always written in sorted order so the file diffs cleanly and is deterministic
//! regardless of resolution order.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const LOCKFILE_FILE_NAME: &str = "dream.lock";
pub const LOCKFILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lockfile {
    pub version: u32,
    #[serde(rename = "package", default)]
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    /// `registry+<url>`, `git+<url>#<rev>`, or `path+<path>`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Resolved dependency names of this locked package, as `"name version"` strings.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl Lockfile {
    pub fn new(mut packages: Vec<LockedPackage>) -> Self {
        packages.sort();
        Lockfile {
            version: LOCKFILE_VERSION,
            packages,
        }
    }

    pub fn load(path: &Path) -> Result<Lockfile> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading lockfile at {}", path.display()))?;
        let lock: Lockfile = toml::from_str(&text)
            .with_context(|| format!("parsing lockfile at {}", path.display()))?;
        Ok(lock)
    }

    pub fn load_if_exists(path: &Path) -> Result<Option<Lockfile>> {
        if path.is_file() {
            Ok(Some(Self::load(path)?))
        } else {
            Ok(None)
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut sorted = self.clone();
        sorted.packages.sort();
        let text = toml::to_string_pretty(&sorted)
            .with_context(|| format!("serializing lockfile for {}", path.display()))?;
        std::fs::write(path, text)
            .with_context(|| format!("writing lockfile at {}", path.display()))?;
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, version: &str) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            version: version.to_string(),
            source: "registry+https://raw.githubusercontent.com/sps014/dream-registry/main".to_string(),
            checksum: Some("sha256:abc".to_string()),
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn new_sorts_packages_deterministically() {
        let lock = Lockfile::new(vec![pkg("zeta", "1.0.0"), pkg("alpha", "1.0.0")]);
        let names: Vec<&str> = lock.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn round_trips_through_toml_and_stays_sorted() {
        let lock = Lockfile::new(vec![pkg("zeta", "1.0.0"), pkg("alpha", "2.0.0")]);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(LOCKFILE_FILE_NAME);
        lock.save(&path).unwrap();

        let loaded = Lockfile::load(&path).unwrap();
        assert_eq!(loaded.version, LOCKFILE_VERSION);
        assert_eq!(loaded.packages.len(), 2);
        assert_eq!(loaded.packages[0].name, "alpha");
        assert_eq!(loaded.find("zeta").unwrap().version, "1.0.0");
    }

    #[test]
    fn load_if_exists_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(LOCKFILE_FILE_NAME);
        assert!(Lockfile::load_if_exists(&path).unwrap().is_none());
    }
}
