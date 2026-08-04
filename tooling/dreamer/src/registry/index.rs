use serde::{Deserialize, Serialize};

/// One published version of a package, as recorded in the registry's per-package index file
/// (one JSON object per line).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub vers: String,
    #[serde(default)]
    pub deps: Vec<IndexDependency>,
    /// `sha256:<hex>` checksum of the tarball.
    pub cksum: String,
    /// Location of the tarball, resolved relative to the registry base URL when not absolute.
    pub tarball: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDependency {
    pub name: String,
    pub req: String,
}
