//! HTTP(S) sparse-index registry client. Talks to any static file server (or a purpose-built
//! registry service) exposing the layout documented in `registry/mod.rs`, plus two optional
//! dynamic endpoints used by `dreamer search`/`dreamer publish`:
//!
//! - `GET  <base>/search?q=<query>`  -> JSON array of [`IndexEntry`]
//! - `POST <base>/api/v1/publish`    -> JSON body `{ "entry": IndexEntry, "tarball_base64": "..." }`
//!
//! No production registry implementing the dynamic endpoints is hosted yet; static-index reads
//! (`fetch_index`/`fetch_tarball`) work against any plain HTTP file server.

use super::checksum;
use super::client::RegistryClient;
use super::index::IndexEntry;
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;

pub struct HttpRegistry {
    base: String,
}

impl HttpRegistry {
    pub fn new(base: String) -> Self {
        HttpRegistry {
            base: base.trim_end_matches('/').to_string(),
        }
    }

    fn tarball_url(&self, tarball: &str) -> String {
        if tarball.starts_with("http://") || tarball.starts_with("https://") {
            tarball.to_string()
        } else {
            format!("{}/{}", self.base, tarball.trim_start_matches('/'))
        }
    }
}

impl RegistryClient for HttpRegistry {
    fn base_url(&self) -> &str {
        &self.base
    }

    fn fetch_index(&self, package: &str) -> Result<Vec<IndexEntry>> {
        let url = format!("{}/index/{}", self.base, package);
        let resp = ureq::get(&url).call();
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) => return Ok(Vec::new()),
            Err(e) => bail!("fetching registry index {}: {}", url, e),
        };
        let text = resp
            .into_string()
            .with_context(|| format!("reading registry index response from {}", url))?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                serde_json::from_str::<IndexEntry>(l)
                    .with_context(|| format!("parsing registry index line from {}", url))
            })
            .collect()
    }

    fn fetch_tarball(&self, entry: &IndexEntry, dest_file: &Path) -> Result<()> {
        let url = self.tarball_url(&entry.tarball);
        let resp = ureq::get(&url)
            .call()
            .with_context(|| format!("downloading tarball {}", url))?;
        let mut bytes = Vec::new();
        resp.into_reader()
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading tarball body from {}", url))?;
        checksum::verify(&bytes, &entry.cksum)
            .with_context(|| format!("verifying tarball for {} {}", entry.name, entry.vers))?;
        if let Some(parent) = dest_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest_file, bytes)
            .with_context(|| format!("writing tarball to {}", dest_file.display()))?;
        Ok(())
    }

    fn search(&self, query: &str) -> Result<Vec<IndexEntry>> {
        let url = format!("{}/search?q={}", self.base, urlencode(query));
        match ureq::get(&url).call() {
            Ok(resp) => {
                let text = resp.into_string().unwrap_or_default();
                Ok(serde_json::from_str(&text).unwrap_or_default())
            }
            // Registries without a search endpoint simply can't be searched remotely.
            Err(_) => Ok(Vec::new()),
        }
    }

    fn publish(&self, entry: &IndexEntry, tarball_path: &Path) -> Result<()> {
        use base64::Engine;
        let bytes = std::fs::read(tarball_path)
            .with_context(|| format!("reading tarball at {}", tarball_path.display()))?;
        let body = serde_json::json!({
            "entry": entry,
            "tarball_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        });
        let url = format!("{}/api/v1/publish", self.base);
        ureq::post(&url)
            .send_json(body)
            .with_context(|| format!("publishing to {}", url))?;
        Ok(())
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
