//! Optional `[[generators]]` entries from a nearby `dream.toml`.

use std::path::{Path, PathBuf};

/// Walks from `entry_file`'s directory upward looking for `dream.toml`; returns generator paths
/// resolved relative to the manifest directory.
pub fn load_manifest_generators(entry_file: &str) -> Vec<String> {
    let start = Path::new(entry_file)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("dream.toml");
        if candidate.is_file() {
            return parse_generators_from_manifest(&candidate, &dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Vec::new()
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
