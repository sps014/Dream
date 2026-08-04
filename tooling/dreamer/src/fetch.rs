//! Global download cache shared across every Dream project on the machine, mirroring Cargo's
//! `~/.cargo/registry` layout:
//!
//! ```text
//! ~/.dream/registry/cache/<name>-<version>.tar.gz   downloaded/verified tarballs
//! ~/.dream/registry/src/<name>-<version>/           extracted package sources
//! ~/.dream/registry/git/<sanitized-url>-<rev>/       cloned git dependencies
//! ```

use crate::registry::{IndexEntry, RegistryClient};
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::path::{Path, PathBuf};
use tar::Archive;

pub fn dream_home() -> PathBuf {
    if let Ok(custom) = std::env::var("DREAM_HOME") {
        return PathBuf::from(custom);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dream")
}

pub fn cache_dir() -> PathBuf {
    dream_home().join("registry").join("cache")
}

pub fn src_dir() -> PathBuf {
    dream_home().join("registry").join("src")
}

pub fn git_dir() -> PathBuf {
    dream_home().join("registry").join("git")
}

/// Downloads (if not already cached) and extracts the tarball for `entry`, returning the
/// directory its contents were extracted into.
pub fn fetch_and_extract(registry: &dyn RegistryClient, entry: &IndexEntry) -> Result<PathBuf> {
    let cache_file = cache_dir().join(format!("{}-{}.tar.gz", entry.name, entry.vers));
    let extract_dir = src_dir().join(format!("{}-{}", entry.name, entry.vers));

    if extract_dir.is_dir() {
        return Ok(extract_dir);
    }

    std::fs::create_dir_all(cache_file.parent().unwrap())?;
    if !cache_file.is_file() {
        registry
            .fetch_tarball(entry, &cache_file)
            .with_context(|| format!("fetching {} {}", entry.name, entry.vers))?;
    } else {
        // Re-verify a pre-existing cache hit; a stale/corrupted cache entry should never be
        // silently trusted.
        let bytes = std::fs::read(&cache_file)?;
        if crate::registry::checksum::verify(&bytes, &entry.cksum).is_err() {
            registry.fetch_tarball(entry, &cache_file)?;
        }
    }

    extract_tarball(&cache_file, &extract_dir)
        .with_context(|| format!("extracting {} {}", entry.name, entry.vers))?;
    Ok(extract_dir)
}

fn extract_tarball(tarball: &Path, dest: &Path) -> Result<()> {
    // `Path::with_extension` would mangle a dotted semver dir name like `greeter-1.0.0` (it treats
    // the trailing `.0` as the "extension"), so build the sibling temp directory name by hand.
    let tmp_name = format!(
        "{}.tmp-extract",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("pkg")
    );
    let tmp_dest = dest.with_file_name(tmp_name);
    let _ = std::fs::remove_dir_all(&tmp_dest);
    std::fs::create_dir_all(&tmp_dest)?;

    let file = std::fs::File::open(tarball)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(&tmp_dest)?;

    if dest.is_dir() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::rename(&tmp_dest, dest)?;
    Ok(())
}

/// Packages `project_dir` (its `dream.toml` plus every file under `src/`) into a `.tar.gz` at
/// `dest_tarball`, returning the raw bytes so the caller can compute a checksum before publishing.
pub fn package_project(project_dir: &Path, dest_tarball: &Path) -> Result<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    if let Some(parent) = dest_tarball.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dest_tarball)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let manifest_path = project_dir.join(crate::manifest::MANIFEST_FILE_NAME);
    builder.append_path_with_name(&manifest_path, crate::manifest::MANIFEST_FILE_NAME)?;

    let src_dir = project_dir.join("src");
    if src_dir.is_dir() {
        builder.append_dir_all("src", &src_dir)?;
    }
    builder.into_inner()?.finish()?;

    std::fs::read(dest_tarball).context("re-reading packaged tarball to checksum it")
}
