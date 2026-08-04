use anyhow::{bail, Result};
use sha2::{Digest, Sha256};

/// Computes the `sha256:<hex>` checksum string used throughout the registry protocol and
/// lockfile (`IndexEntry::cksum`, `LockedPackage::checksum`).
pub fn sha256_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

/// Verifies `bytes` matches `expected` (a `sha256:<hex>` string), erroring with both checksums on
/// mismatch so a corrupted/tampered download is never silently accepted.
pub fn verify(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = sha256_of(bytes);
    if actual != expected {
        bail!(
            "checksum mismatch: expected {}, got {}",
            expected,
            actual
        );
    }
    Ok(())
}
