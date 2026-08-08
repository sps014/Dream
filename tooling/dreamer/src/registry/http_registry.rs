//! HTTP(S) sparse-index registry client. Talks to any static file server (or a purpose-built
//! registry service) exposing the layout documented in `registry/mod.rs`, plus optional
//! dynamic endpoints:
//!
//! - `GET  <base>/search?q=<query>`  -> JSON array of [`IndexEntry`]
//! - `GET  <base>/catalog.json`      -> fallback search catalog for static registries
//! - `POST <base>/api/v1/publish`    -> JSON body `{ "entry": IndexEntry, "tarball_base64": "..." }`
//!
//! Static-index reads (`fetch_index`/`fetch_tarball`) work against any plain HTTP file server,
//! including GitHub raw/Pages hosting.

use super::checksum;
use super::client::RegistryClient;
use super::index::IndexEntry;
use super::{CatalogEntry, MAX_TARBALL_BYTES};
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
        // GitHub raw CDN can serve stale index bodies for several minutes after publish;
        // ask for a revalidated response so `dreamer update` sees newly published versions.
        let resp = ureq::get(&url)
            .set("Cache-Control", "no-cache")
            .set("Pragma", "no-cache")
            .call();
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
        if let Ok(resp) = ureq::get(&url).call() {
            let text = resp.into_string().unwrap_or_default();
            if let Ok(entries) = serde_json::from_str::<Vec<IndexEntry>>(&text) {
                return Ok(entries);
            }
        }
        self.search_catalog(query)
    }

    fn publish(&self, entry: &IndexEntry, tarball_path: &Path) -> Result<()> {
        use base64::Engine;
        let bytes = std::fs::read(tarball_path)
            .with_context(|| format!("reading tarball at {}", tarball_path.display()))?;
        if bytes.len() > MAX_TARBALL_BYTES {
            bail!(
                "tarball is {} bytes; registry limit is {} bytes (10 MiB)",
                bytes.len(),
                MAX_TARBALL_BYTES
            );
        }
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

impl HttpRegistry {
    fn search_catalog(&self, query: &str) -> Result<Vec<IndexEntry>> {
        let url = format!("{}/catalog.json", self.base);
        let resp = match ureq::get(&url).call() {
            Ok(r) => r,
            Err(_) => return Ok(Vec::new()),
        };
        let text = resp.into_string().unwrap_or_default();
        let catalog: Vec<CatalogEntry> = serde_json::from_str(&text).unwrap_or_default();
        let needle = query.to_lowercase();
        let mut out: Vec<IndexEntry> = catalog
            .into_iter()
            .filter(|c| c.matches_query(&needle))
            .map(|c| IndexEntry {
                name: c.name,
                vers: c.vers,
                description: c.description,
                authors: c.authors,
                license: c.license,
                edition: c.edition,
                package_type: c.package_type,
                targets: c.targets,
                readme: c.readme,
                keywords: c.keywords,
                ..Default::default()
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
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
