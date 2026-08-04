//! Local-filesystem registry: a plain directory implementing the sparse-index protocol. Used for
//! private/offline registries (`[registries] default = "file:///path/to/registry"`) and as the
//! fixture backing `tooling/dreamer/tests/` — no server process required.

use super::checksum;
use super::client::RegistryClient;
use super::index::IndexEntry;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct FileRegistry {
    base: PathBuf,
}

impl FileRegistry {
    pub fn new(base: PathBuf) -> Self {
        FileRegistry { base }
    }

    fn index_file(&self, package: &str) -> PathBuf {
        self.base.join("index").join(package)
    }

    /// Resolves a (possibly relative) tarball location from an [`IndexEntry`] against the
    /// registry base directory.
    fn tarball_path(&self, tarball: &str) -> PathBuf {
        let p = Path::new(tarball);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.base.join(p)
        }
    }
}

impl RegistryClient for FileRegistry {
    fn base_url(&self) -> &str {
        self.base.to_str().unwrap_or_default()
    }

    fn fetch_index(&self, package: &str) -> Result<Vec<IndexEntry>> {
        let path = self.index_file(package);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading registry index at {}", path.display()))?;
        parse_index_lines(&text)
    }

    fn fetch_tarball(&self, entry: &IndexEntry, dest_file: &Path) -> Result<()> {
        let src = self.tarball_path(&entry.tarball);
        let bytes =
            std::fs::read(&src).with_context(|| format!("reading tarball at {}", src.display()))?;
        checksum::verify(&bytes, &entry.cksum)
            .with_context(|| format!("verifying tarball for {} {}", entry.name, entry.vers))?;
        if let Some(parent) = dest_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest_file, bytes)
            .with_context(|| format!("writing tarball to {}", dest_file.display()))?;
        Ok(())
    }

    fn search(&self, query: &str) -> Result<Vec<IndexEntry>> {
        let index_dir = self.base.join("index");
        if !index_dir.is_dir() {
            return Ok(Vec::new());
        }
        let needle = query.to_lowercase();
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&index_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if needle.is_empty() || name.to_lowercase().contains(&needle) {
                if let Some(latest) = self.fetch_index(&name)?.pop() {
                    out.push(latest);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn publish(&self, entry: &IndexEntry, tarball_path: &Path) -> Result<()> {
        let index_path = self.index_file(&entry.name);
        std::fs::create_dir_all(index_path.parent().unwrap())?;
        let mut existing = self.fetch_index(&entry.name)?;
        if existing.iter().any(|e| e.vers == entry.vers) {
            anyhow::bail!(
                "{} {} is already published to {}",
                entry.name,
                entry.vers,
                self.base.display()
            );
        }
        existing.push(entry.clone());
        let text = existing
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .context("serializing index entries")?
            .join("\n")
            + "\n";
        std::fs::write(&index_path, text)
            .with_context(|| format!("writing registry index at {}", index_path.display()))?;

        let dest = self.tarball_path(&entry.tarball);
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::copy(tarball_path, &dest).with_context(|| {
            format!(
                "copying tarball {} to {}",
                tarball_path.display(),
                dest.display()
            )
        })?;
        Ok(())
    }
}

fn parse_index_lines(text: &str) -> Result<Vec<IndexEntry>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<IndexEntry>(l).context("parsing registry index line"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::checksum;

    #[test]
    fn publish_then_fetch_index_and_tarball_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(tmp.path().to_path_buf());

        let tarball_src = tmp.path().join("staging.tar.gz");
        std::fs::write(&tarball_src, b"fake tarball bytes").unwrap();
        let cksum = checksum::sha256_of(b"fake tarball bytes");

        let entry = IndexEntry {
            name: "json-tools".to_string(),
            vers: "0.3.1".to_string(),
            deps: Vec::new(),
            cksum,
            tarball: "dl/json-tools/json-tools-0.3.1.tar.gz".to_string(),
            description: Some("JSON helpers".to_string()),
        };

        registry.publish(&entry, &tarball_src).unwrap();

        let fetched = registry.fetch_index("json-tools").unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].vers, "0.3.1");

        let dest = tmp.path().join("downloaded.tar.gz");
        registry.fetch_tarball(&fetched[0], &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"fake tarball bytes");
    }

    #[test]
    fn fetch_tarball_rejects_corrupted_download() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(tmp.path().to_path_buf());

        let tarball_src = tmp.path().join("staging.tar.gz");
        std::fs::write(&tarball_src, b"original bytes").unwrap();
        let entry = IndexEntry {
            name: "pkg".to_string(),
            vers: "1.0.0".to_string(),
            deps: Vec::new(),
            cksum: checksum::sha256_of(b"original bytes"),
            tarball: "dl/pkg/pkg-1.0.0.tar.gz".to_string(),
            description: None,
        };
        registry.publish(&entry, &tarball_src).unwrap();

        // Corrupt the stored tarball after publishing.
        std::fs::write(tmp.path().join("dl/pkg/pkg-1.0.0.tar.gz"), b"tampered!").unwrap();

        let dest = tmp.path().join("downloaded.tar.gz");
        assert!(registry.fetch_tarball(&entry, &dest).is_err());
    }

    #[test]
    fn publish_rejects_duplicate_version() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(tmp.path().to_path_buf());
        let tarball_src = tmp.path().join("staging.tar.gz");
        std::fs::write(&tarball_src, b"bytes").unwrap();
        let entry = IndexEntry {
            name: "pkg".to_string(),
            vers: "1.0.0".to_string(),
            deps: Vec::new(),
            cksum: checksum::sha256_of(b"bytes"),
            tarball: "dl/pkg/pkg-1.0.0.tar.gz".to_string(),
            description: None,
        };
        registry.publish(&entry, &tarball_src).unwrap();
        assert!(registry.publish(&entry, &tarball_src).is_err());
    }

    #[test]
    fn fetch_index_of_unpublished_package_is_empty_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(tmp.path().to_path_buf());
        assert!(registry.fetch_index("never-published").unwrap().is_empty());
    }
}
