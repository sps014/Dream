//! Optional `[[generators]]` entries from a nearby `dream.toml`.

use std::path::{Path, PathBuf};

/// Walks from `entry_file`'s directory upward looking for `dream.toml`.
pub fn find_project_root(entry_file: &str) -> Option<PathBuf> {
    let start = Path::new(entry_file)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("dream.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Cache directory for a generator harness: `target/generators/<kind>-<fingerprint>` when a
/// project root is known, otherwise the system temp dir.
pub fn harness_cache_dir(entry_file: Option<&str>, kind: &str, fingerprint: u64) -> PathBuf {
    let name = format!("dream-{kind}-{fingerprint:x}");
    if let Some(entry) = entry_file {
        if let Some(root) = find_project_root(entry) {
            return root.join("target").join("generators").join(name);
        }
    }
    std::env::temp_dir().join(name)
}

/// Walks from `entry_file`'s directory upward looking for `dream.toml`; returns generator paths
/// resolved relative to the manifest directory.
pub fn load_manifest_generators(entry_file: &str) -> Vec<String> {
    let Some(dir) = find_project_root(entry_file) else {
        return Vec::new();
    };
    let candidate = dir.join("dream.toml");
    parse_generators_from_manifest(&candidate, &dir)
}

/// Minimal extraction of `[[generators]]` `path = "..."` entries (no full TOML dependency).
fn parse_generators_from_manifest(manifest: &Path, base: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_generators = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_generators = trimmed == "[[generators]]";
            continue;
        }
        if !in_generators {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("path") {
            let rest = rest.trim().trim_start_matches('=').trim();
            let path = rest.trim_matches('"').trim_matches('\'');
            if path.is_empty() {
                continue;
            }
            let resolved: PathBuf = base.join(path);
            if let Ok(canon) = resolved.canonicalize() {
                if let Some(s) = canon.to_str() {
                    out.push(s.to_string());
                }
            } else if let Some(s) = resolved.to_str() {
                out.push(s.to_string());
            }
        }
    }
    out
}
