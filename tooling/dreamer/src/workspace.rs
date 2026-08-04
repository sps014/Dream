//! Project discovery (`dream.toml` lookup) and `dream_packages/` materialization: takes a
//! resolved dependency graph and lays each package out on disk where the compiler's import
//! resolution (`src/driver/source_loader.rs`) expects to find it.

use crate::fetch;
use crate::lockfile::{Lockfile, LockedPackage, LOCKFILE_FILE_NAME};
use crate::manifest::{import_segment, Manifest, MANIFEST_FILE_NAME};
use crate::registry::open_registry;
use crate::resolver::{ResolvedPackage, ResolvedSource};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The directory dependency sources are materialized into, sitting next to `dream.toml`. Never
/// committed to version control (see `dreamer init`'s generated `.gitignore`) since its contents
/// are fully reproducible from `dream.toml` + `dream.lock`.
pub const PACKAGES_DIR_NAME: &str = "dream_packages";

pub struct Workspace {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Workspace {
    pub fn discover(start_dir: &Path) -> Result<Workspace> {
        let root = Manifest::find_project_root(start_dir).with_context(|| {
            format!(
                "no {} found in {} or any parent directory",
                MANIFEST_FILE_NAME,
                start_dir.display()
            )
        })?;
        let manifest = Manifest::load(&root.join(MANIFEST_FILE_NAME))?;
        Ok(Workspace { root, manifest })
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE_NAME)
    }

    pub fn lockfile_path(&self) -> PathBuf {
        self.root.join(LOCKFILE_FILE_NAME)
    }

    pub fn packages_dir(&self) -> PathBuf {
        self.root.join(PACKAGES_DIR_NAME)
    }

    pub fn entry_path(&self) -> PathBuf {
        self.root.join(&self.manifest.package.entry)
    }

    pub fn save_manifest(&self) -> Result<()> {
        self.manifest.save(&self.manifest_path())
    }

    /// Materializes every resolved package into `dream_packages/<import-segment-name>/`, and
    /// returns the lockfile that should be written to disk to pin this exact resolution.
    pub fn install(&self, resolved: &[ResolvedPackage]) -> Result<Lockfile> {
        let packages_dir = self.packages_dir();
        std::fs::create_dir_all(&packages_dir)?;

        let by_name: BTreeMap<&str, &ResolvedPackage> =
            resolved.iter().map(|p| (p.name.as_str(), p)).collect();

        let mut locked = Vec::with_capacity(resolved.len());
        for pkg in resolved {
            let source_dir = match &pkg.source {
                ResolvedSource::Path { path } => path.clone(),
                ResolvedSource::Git { checkout_dir, .. } => checkout_dir.clone(),
                ResolvedSource::Registry { url, tarball, checksum } => {
                    let entry = crate::registry::IndexEntry {
                        name: pkg.name.clone(),
                        vers: pkg.version.clone(),
                        deps: Vec::new(),
                        cksum: checksum.clone(),
                        tarball: tarball.clone(),
                        description: None,
                    };
                    let client = open_registry(url);
                    fetch::fetch_and_extract(client.as_ref(), &entry)?
                }
            };

            let dest = packages_dir.join(import_segment(&pkg.name));
            link_or_copy_dir(&source_dir, &dest).with_context(|| {
                format!(
                    "installing '{}' into {}",
                    pkg.name,
                    dest.display()
                )
            })?;

            let dependencies = pkg
                .dependencies
                .iter()
                .filter_map(|dep_name| {
                    by_name
                        .get(dep_name.as_str())
                        .map(|dep| format!("{} {}", dep.name, dep.version))
                })
                .collect();

            locked.push(LockedPackage {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                source: pkg.source.lock_source(),
                checksum: match &pkg.source {
                    ResolvedSource::Registry { checksum, .. } => Some(checksum.clone()),
                    _ => None,
                },
                dependencies,
            });
        }

        Ok(Lockfile::new(locked))
    }
}

/// Replaces `dest` with a fresh view of `src`'s contents: a symlink where the platform/filesystem
/// allows it (so edits to a local `path` dependency show up immediately), falling back to a
/// recursive copy (used for registry/git sources, and anywhere symlinks aren't permitted).
fn link_or_copy_dir(src: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        if dest.is_symlink() {
            std::fs::remove_file(dest)?;
        } else {
            std::fs::remove_dir_all(dest)?;
        }
    }

    if try_symlink(src, dest).is_ok() {
        return Ok(());
    }

    copy_dir_recursive(src, dest)
}

#[cfg(unix)]
fn try_symlink(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dest)
}

#[cfg(windows)]
fn try_symlink(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dest)
}

#[cfg(not(any(unix, windows)))]
fn try_symlink(_src: &Path, _dest: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks not supported on this platform",
    ))
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}
