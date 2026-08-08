//! GitHub-repo registry: reads over static HTTPS (`raw.githubusercontent.com` or GitHub Pages),
//! publishes via the GitHub Contents API so a plain public repo can host `index/` + `dl/` with
//! no custom server.

use super::checksum;
use super::client::RegistryClient;
use super::http_registry::HttpRegistry;
use super::index::IndexEntry;
use super::{CatalogEntry, MAX_TARBALL_BYTES};
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::Deserialize;
use std::path::Path;

/// Public HTTPS base + GitHub API coordinates for a registry hosted as a git repository.
pub struct GithubRegistry {
    http: HttpRegistry,
    owner: String,
    repo: String,
    branch: String,
    token: Option<String>,
}

impl GithubRegistry {
    pub fn try_parse(url: &str, token: Option<String>) -> Option<Self> {
        let (owner, repo, branch) = parse_github_registry_url(url)?;
        Some(GithubRegistry {
            http: HttpRegistry::new(url.to_string()),
            owner,
            repo,
            branch,
            token,
        })
    }

    fn api_contents_url(&self, path: &str) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
            self.owner,
            self.repo,
            path.trim_start_matches('/'),
            self.branch
        )
    }

    fn api_put_url(&self, path: &str) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/contents/{}",
            self.owner,
            self.repo,
            path.trim_start_matches('/')
        )
    }

    fn auth_headers(&self, mut req: ureq::Request) -> Result<ureq::Request> {
        let token = self
            .token
            .as_deref()
            .filter(|t| !t.is_empty())
            .context(
                "publishing to a GitHub registry requires a token \
                 (set DREAM_REGISTRY_TOKEN or pass --token)",
            )?;
        req = req
            .set("Authorization", &format!("Bearer {}", token))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "dreamer")
            .set("X-GitHub-Api-Version", "2022-11-28");
        Ok(req)
    }

    fn get_file(&self, path: &str) -> Result<Option<GhContent>> {
        let url = self.api_contents_url(path);
        let req = ureq::get(&url)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "dreamer")
            .set("X-GitHub-Api-Version", "2022-11-28");
        let req = if let Some(token) = self.token.as_deref().filter(|t| !t.is_empty()) {
            req.set("Authorization", &format!("Bearer {}", token))
        } else {
            req
        };
        match req.call() {
            Ok(resp) => {
                let body: GhContent = resp
                    .into_json()
                    .with_context(|| format!("parsing GitHub contents response for {}", path))?;
                Ok(Some(body))
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(e) => bail!("fetching GitHub contents {}: {}", url, e),
        }
    }

    fn put_file(&self, path: &str, bytes: &[u8], message: &str) -> Result<()> {
        let existing_sha = self.get_file(path)?.map(|c| c.sha);
        let mut body = serde_json::json!({
            "message": message,
            "content": base64::engine::general_purpose::STANDARD.encode(bytes),
            "branch": self.branch,
        });
        if let Some(sha) = existing_sha {
            body["sha"] = serde_json::Value::String(sha);
        }
        let url = self.api_put_url(path);
        let req = self.auth_headers(ureq::put(&url))?;
        match req.send_json(body) {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, resp)) => {
                let detail = resp.into_string().unwrap_or_default();
                bail!(
                    "GitHub Contents API PUT {} failed (HTTP {}): {}",
                    path,
                    code,
                    detail
                );
            }
            Err(e) => bail!("GitHub Contents API PUT {}: {}", path, e),
        }
    }

    fn update_catalog(&self, entry: &IndexEntry) -> Result<()> {
        let path = "catalog.json";
        let mut catalog: Vec<CatalogEntry> = match self.get_file(path)? {
            Some(content) => {
                let raw = content.decoded()?;
                serde_json::from_slice(&raw).unwrap_or_default()
            }
            None => Vec::new(),
        };
        catalog.retain(|c| c.name != entry.name);
        catalog.push(CatalogEntry::from_index(entry));
        catalog.sort_by(|a, b| a.name.cmp(&b.name));
        let text = serde_json::to_vec_pretty(&catalog).context("serializing catalog.json")?;
        self.put_file(
            path,
            &text,
            &format!("registry: update catalog for {} {}", entry.name, entry.vers),
        )
    }
}

impl RegistryClient for GithubRegistry {
    fn base_url(&self) -> &str {
        self.http.base_url()
    }

    fn fetch_index(&self, package: &str) -> Result<Vec<IndexEntry>> {
        self.http.fetch_index(package)
    }

    fn fetch_tarball(&self, entry: &IndexEntry, dest_file: &Path) -> Result<()> {
        self.http.fetch_tarball(entry, dest_file)
    }

    fn search(&self, query: &str) -> Result<Vec<IndexEntry>> {
        self.http.search(query)
    }

    fn publish(&self, entry: &IndexEntry, tarball_path: &Path) -> Result<()> {
        let bytes = std::fs::read(tarball_path)
            .with_context(|| format!("reading tarball at {}", tarball_path.display()))?;
        if bytes.len() > MAX_TARBALL_BYTES {
            bail!(
                "tarball is {} bytes; registry limit is {} bytes (10 MiB)",
                bytes.len(),
                MAX_TARBALL_BYTES
            );
        }
        checksum::verify(&bytes, &entry.cksum)
            .context("tarball checksum does not match index entry")?;

        let existing = self.fetch_index(&entry.name)?;
        if existing.iter().any(|e| e.vers == entry.vers) {
            bail!(
                "{} {} is already published to {}",
                entry.name,
                entry.vers,
                self.base_url()
            );
        }

        self.put_file(
            &entry.tarball,
            &bytes,
            &format!("registry: add {} {}", entry.name, entry.vers),
        )?;

        let mut entries = existing;
        entries.push(entry.clone());
        let index_body = entries
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .context("serializing index entries")?
            .join("\n")
            + "\n";
        self.put_file(
            &format!("index/{}", entry.name),
            index_body.as_bytes(),
            &format!("registry: index {} {}", entry.name, entry.vers),
        )?;

        self.update_catalog(entry)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct GhContent {
    sha: String,
    encoding: Option<String>,
    content: Option<String>,
}

impl GhContent {
    fn decoded(&self) -> Result<Vec<u8>> {
        let content = self
            .content
            .as_deref()
            .context("GitHub contents response missing content")?;
        let cleaned: String = content.chars().filter(|c| !c.is_whitespace()).collect();
        if self.encoding.as_deref() == Some("base64") || self.encoding.is_none() {
            base64::engine::general_purpose::STANDARD
                .decode(cleaned)
                .context("decoding GitHub file content")
        } else {
            Ok(cleaned.into_bytes())
        }
    }
}

/// Parses a static GitHub-hosted registry base URL into `(owner, repo, branch)`.
pub fn parse_github_registry_url(url: &str) -> Option<(String, String, String)> {
    let url = url.trim().trim_end_matches('/');
    if let Some(rest) = url.strip_prefix("https://raw.githubusercontent.com/") {
        let mut parts = rest.split('/');
        let owner = parts.next()?.to_string();
        let repo = parts.next()?.to_string();
        let branch = parts.next()?.to_string();
        if parts.next().is_some() || owner.is_empty() || repo.is_empty() || branch.is_empty() {
            return None;
        }
        return Some((owner, repo, branch));
    }
    if let Some(rest) = url.strip_prefix("https://") {
        let (host, path) = rest.split_once('/')?;
        let owner = host.strip_suffix(".github.io")?;
        let mut segs = path.split('/').filter(|s| !s.is_empty());
        let repo = segs.next()?;
        if segs.next().is_some() || owner.is_empty() || repo.is_empty() {
            return None;
        }
        return Some((owner.to_string(), repo.to_string(), "main".to_string()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raw_githubusercontent_base() {
        let (o, r, b) =
            parse_github_registry_url("https://raw.githubusercontent.com/sps014/dream-registry/main")
                .unwrap();
        assert_eq!(o, "sps014");
        assert_eq!(r, "dream-registry");
        assert_eq!(b, "main");
    }

    #[test]
    fn parses_github_pages_base() {
        let (o, r, b) = parse_github_registry_url("https://sps014.github.io/dream-registry").unwrap();
        assert_eq!(o, "sps014");
        assert_eq!(r, "dream-registry");
        assert_eq!(b, "main");
    }

    #[test]
    fn rejects_deeper_raw_paths() {
        assert!(parse_github_registry_url(
            "https://raw.githubusercontent.com/sps014/dream-registry/main/index/foo"
        )
        .is_none());
    }
}
