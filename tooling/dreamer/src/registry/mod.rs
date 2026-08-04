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
//! ```

pub mod checksum;
mod client;
mod file_registry;
mod http_registry;
mod index;

pub use client::RegistryClient;
pub use index::{IndexDependency, IndexEntry};

/// Fallback registry base URL used when a dependency (or `[registries] default`) doesn't specify
/// one. Placeholder: no production Dream registry is hosted yet, but the client is agnostic to
/// the concrete URL — point `[registries] default` at any `file://` or `http(s)://` location
/// implementing the protocol documented above.
pub const DEFAULT_REGISTRY: &str = "https://registry.dream-lang.org";

/// Opens a [`RegistryClient`] for `url`, dispatching on scheme: `file://`/bare paths use the
/// local-filesystem implementation (handy for private registries and for tests — see
/// `tooling/dreamer/tests/`), anything else is treated as an HTTP(S) sparse index.
pub fn open_registry(url: &str) -> Box<dyn RegistryClient> {
    if let Some(path) = url.strip_prefix("file://") {
        Box::new(file_registry::FileRegistry::new(path.into()))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Box::new(http_registry::HttpRegistry::new(url.to_string()))
    } else {
        // Bare filesystem path, e.g. from a relative `[registries]` entry in tests/fixtures.
        Box::new(file_registry::FileRegistry::new(url.into()))
    }
}
