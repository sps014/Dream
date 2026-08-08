//! Registry protocol: a per-package sparse index (JSON-lines, one line per published version)
//! plus tarball downloads, modeled after crates.io's sparse index / npm registry so no bespoke
//! server needs to be written to try this out — a plain directory served over `file://` (or any
//! static file server over `http(s)://`) is a fully compliant registry.
//!
//! Index layout, rooted at a registry's base URL:
//!
//! ```text
//! <base>/index/<name>        newline-delimited JSON, one IndexEntry per published version
//! <base>/dl/<name>/<name>-<version>.tar.gz   tarball referenced by IndexEntry::tarball
//! <base>/catalog.json        compact search catalog (optional; used when /search is absent)
//! ```

pub mod checksum;
mod client;
mod file_registry;
mod github_registry;
mod http_registry;
mod index;

pub use client::RegistryClient;
pub use index::{IndexDependency, IndexEntry};

use serde::{Deserialize, Serialize};

/// Maximum published package tarball size (10 MiB). Enforced by dreamer and by GitHub-registry
/// publish before uploading.
pub const MAX_TARBALL_BYTES: usize = 10 * 1024 * 1024;

/// Default public registry: sparse index + tarballs hosted in the
/// [`sps014/dream-registry`](https://github.com/sps014/dream-registry) GitHub repository.
pub const DEFAULT_REGISTRY: &str =
    "https://raw.githubusercontent.com/sps014/dream-registry/main";

/// One row in `catalog.json` for static-registry search (latest published metadata per package).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub vers: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    #[serde(
        default,
        rename = "type",
        skip_serializing_if = "Option::is_none"
    )]
    pub package_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    /// Archive-relative README path inside the package tarball (e.g. `README.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

impl CatalogEntry {
    pub fn from_index(entry: &IndexEntry) -> Self {
        CatalogEntry {
            name: entry.name.clone(),
            vers: entry.vers.clone(),
            description: entry.description.clone(),
            authors: entry.authors.clone(),
            license: entry.license.clone(),
            edition: entry.edition.clone(),
            package_type: entry.package_type.clone(),
            targets: entry.targets.clone(),
            readme: entry.readme.clone(),
            keywords: entry.keywords.clone(),
        }
    }

    /// Case-insensitive substring match against name, description, and keywords.
    pub fn matches_query(&self, query: &str) -> bool {
        IndexEntry {
            name: self.name.clone(),
            description: self.description.clone(),
            keywords: self.keywords.clone(),
            ..Default::default()
        }
        .matches_query(query)
    }
}

/// Resolves a publish token: explicit CLI value, then `DREAM_REGISTRY_TOKEN`, then `GITHUB_TOKEN`.
pub fn registry_token(explicit: Option<String>) -> Option<String> {
    explicit
        .filter(|t| !t.is_empty())
        .or_else(|| std::env::var("DREAM_REGISTRY_TOKEN").ok().filter(|t| !t.is_empty()))
        .or_else(|| std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()))
}

/// Opens a [`RegistryClient`] for `url`. GitHub-hosted static registries
/// (`raw.githubusercontent.com/...` or `*.github.io/...`) use the Contents API for publish;
/// other `http(s)://` URLs use the generic HTTP client; `file://` / bare paths use the
/// local-filesystem implementation.
pub fn open_registry(url: &str) -> Box<dyn RegistryClient> {
    open_registry_with_token(url, registry_token(None))
}

/// Like [`open_registry`], but uses `token` for GitHub Contents API publishes when provided
/// (otherwise falls back to env vars via [`registry_token`]).
pub fn open_registry_with_token(url: &str, token: Option<String>) -> Box<dyn RegistryClient> {
    let token = token
        .filter(|t| !t.is_empty())
        .or_else(|| registry_token(None));
    if let Some(gh) = github_registry::GithubRegistry::try_parse(url, token) {
        return Box::new(gh);
    }
    if let Some(path) = url.strip_prefix("file://") {
        Box::new(file_registry::FileRegistry::new(path.into()))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Box::new(http_registry::HttpRegistry::new(url.to_string()))
    } else {
        Box::new(file_registry::FileRegistry::new(url.into()))
    }
}
