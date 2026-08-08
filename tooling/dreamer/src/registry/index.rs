use serde::{Deserialize, Serialize};

/// One published version of a package, as recorded in the registry's per-package index file
/// (one JSON object per line).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexEntry {
    pub name: String,
    pub vers: String,
    #[serde(default)]
    pub deps: Vec<IndexDependency>,
    /// `sha256:<hex>` checksum of the tarball.
    #[serde(default)]
    pub cksum: String,
    /// Location of the tarball, resolved relative to the registry base URL when not absolute.
    #[serde(default)]
    pub tarball: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    /// `bin` or `lib`, matching `[package].type` in `dream.toml`.
    #[serde(
        default,
        rename = "type",
        skip_serializing_if = "Option::is_none"
    )]
    pub package_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    /// Archive-relative path to the README inside the published tarball (e.g. `README.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

impl IndexEntry {
    /// Case-insensitive substring match against name, description, and keywords.
    pub fn matches_query(&self, query: &str) -> bool {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        if self.name.to_lowercase().contains(&needle) {
            return true;
        }
        if self
            .description
            .as_ref()
            .is_some_and(|d| d.to_lowercase().contains(&needle))
        {
            return true;
        }
        self.keywords
            .iter()
            .any(|k| k.to_lowercase().contains(&needle))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDependency {
    pub name: String,
    pub req: String,
}
