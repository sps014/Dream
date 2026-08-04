use super::index::IndexEntry;
use anyhow::Result;
use std::path::Path;

/// A registry backend able to answer the sparse-index protocol (see `registry/mod.rs`).
/// Implemented for both HTTP(S) and local-filesystem (`file://`) registries so the resolver and
/// commands never need to know which kind they're talking to.
pub trait RegistryClient {
    /// Base URL/path this client was opened with (used to record `source = "registry+<url>"` in
    /// the lockfile).
    fn base_url(&self) -> &str;

    /// Every published version of `package`, in the order the registry stored them (newest last,
    /// by convention). Returns an empty vec (not an error) if the package has never been
    /// published, so callers can produce a clear "no matching version" error themselves.
    fn fetch_index(&self, package: &str) -> Result<Vec<IndexEntry>>;

    /// Downloads (or copies) the tarball for `entry` into `dest_file`, verifying its `cksum`.
    fn fetch_tarball(&self, entry: &IndexEntry, dest_file: &Path) -> Result<()>;

    /// Best-effort substring search over package names for `dreamer search`. Registries that
    /// don't support search may return an empty vec.
    fn search(&self, query: &str) -> Result<Vec<IndexEntry>>;

    /// Publishes a new version by appending `entry` to the package's index and storing
    /// `tarball_path` at the location `entry.tarball` names.
    fn publish(&self, entry: &IndexEntry, tarball_path: &Path) -> Result<()>;
}
