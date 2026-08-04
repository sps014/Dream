//! Dependency resolution: turns a manifest's `[dependencies]`/`[dev-dependencies]` into a
//! concrete, versioned dependency graph.
//!
//! Registry dependencies use a greedy "highest version satisfying every accumulated requirement"
//! strategy — the same class of algorithm as Cargo's classic (non-`-Zminimal-versions`) resolver:
//! requirements accumulate breadth-first (a package's own registry dependencies contribute new
//! requirements once *it* is resolved), and the loop repeats until a fixed point is reached. This
//! intentionally does not attempt full PubGrub-style backtracking; diamond dependencies resolve
//! correctly as long as a single version can satisfy every accumulated requirement, and conflicts
//! are reported clearly rather than silently picked around.
//!
//! Path and git dependencies are resolved immediately by reading the dependency's own
//! `dream.toml` and are treated as pinned (never subject to registry version selection).

use crate::manifest::{Dependency, Manifest, MANIFEST_FILE_NAME};
use crate::registry::{open_registry, IndexEntry};
use anyhow::{anyhow, bail, Context, Result};
use semver::{Version, VersionReq};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum ResolvedSource {
    Registry {
        url: String,
        tarball: String,
        checksum: String,
    },
    Path {
        path: PathBuf,
    },
    Git {
        url: String,
        checkout_dir: PathBuf,
        rev: Option<String>,
        tag: Option<String>,
        branch: Option<String>,
    },
}

impl ResolvedSource {
    /// The `source` string recorded in `dream.lock`.
    pub fn lock_source(&self) -> String {
        match self {
            ResolvedSource::Registry { url, .. } => format!("registry+{}", url),
            ResolvedSource::Path { path } => format!("path+{}", path.display()),
            ResolvedSource::Git {
                url,
                rev,
                tag,
                branch,
                ..
            } => {
                let checkout = rev.as_deref().or(tag.as_deref()).or(branch.as_deref());
                match checkout {
                    Some(c) => format!("git+{}#{}", url, c),
                    None => format!("git+{}", url),
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
    pub source: ResolvedSource,
    /// Names of this package's own dependencies (versions filled in by the caller once the whole
    /// graph is known).
    pub dependencies: Vec<String>,
}

/// Resolves `manifest`'s dependency graph. `preferred` is an optional map of package name ->
/// version (typically sourced from an existing `dream.lock`): when a registry package has
/// multiple versions satisfying every accumulated requirement, the preferred version is kept
/// instead of jumping to the newest match, so re-running `install` doesn't silently upgrade
/// dependencies just because a newer version was published.
pub fn resolve(
    manifest: &Manifest,
    project_dir: &Path,
    include_dev: bool,
    preferred: &BTreeMap<String, String>,
) -> Result<Vec<ResolvedPackage>> {
    let mut resolver = Resolver {
        preferred: preferred.clone(),
        ..Resolver::default()
    };
    for (name, dep) in manifest.all_dependencies(include_dev) {
        resolver.queue_dependency(&name, &dep, project_dir, manifest)?;
    }
    resolver.run()?;
    Ok(resolver.resolved.into_values().collect())
}

#[derive(Default)]
struct Resolver {
    resolved: BTreeMap<String, ResolvedPackage>,
    requirements: BTreeMap<String, Vec<(VersionReq, String)>>,
    index_cache: HashMap<(String, String), Vec<IndexEntry>>,
    visited_dirs: HashSet<PathBuf>,
    preferred: BTreeMap<String, String>,
}

impl Resolver {
    fn queue_dependency(
        &mut self,
        name: &str,
        dep: &Dependency,
        base_dir: &Path,
        manifest: &Manifest,
    ) -> Result<()> {
        if let Some(rel_path) = dep.path() {
            let dep_dir = base_dir.join(rel_path).canonicalize().with_context(|| {
                format!("resolving path dependency '{}' at '{}'", name, rel_path)
            })?;
            self.queue_local_project(dep_dir, ResolvedKind::Path)?;
            return Ok(());
        }

        if let Some(git_url) = dep.git() {
            let d = dep.detailed();
            let checkout_dir = crate::git::fetch_git_dependency(
                git_url,
                d.tag.as_deref(),
                d.branch.as_deref(),
                d.rev.as_deref(),
            )
            .with_context(|| format!("fetching git dependency '{}' from '{}'", name, git_url))?;
            self.queue_local_project(
                checkout_dir.clone(),
                ResolvedKind::Git {
                    url: git_url.to_string(),
                    rev: d.rev.clone(),
                    tag: d.tag.clone(),
                    branch: d.branch.clone(),
                },
            )?;
            return Ok(());
        }

        let req_str = dep.version_req().unwrap_or("*");
        let req = VersionReq::parse(req_str)
            .with_context(|| format!("parsing version requirement '{}' for '{}'", req_str, name))?;
        let url = manifest
            .registry_url(dep.registry_alias())
            .ok_or_else(|| anyhow!("no registry configured for dependency '{}'", name))?;
        self.requirements
            .entry(name.to_string())
            .or_default()
            .push((req, url));
        Ok(())
    }

    fn queue_local_project(&mut self, dir: PathBuf, kind: ResolvedKind) -> Result<()> {
        if !self.visited_dirs.insert(dir.clone()) {
            return Ok(());
        }
        let manifest_path = dir.join(MANIFEST_FILE_NAME);
        let dep_manifest = Manifest::load(&manifest_path).with_context(|| {
            format!("loading manifest for local dependency at {}", dir.display())
        })?;

        let source = match kind {
            ResolvedKind::Path => ResolvedSource::Path { path: dir.clone() },
            ResolvedKind::Git {
                url,
                rev,
                tag,
                branch,
            } => ResolvedSource::Git {
                url,
                checkout_dir: dir.clone(),
                rev,
                tag,
                branch,
            },
        };

        let child_deps = dep_manifest.all_dependencies(false);
        self.resolved.insert(
            dep_manifest.package.name.clone(),
            ResolvedPackage {
                name: dep_manifest.package.name.clone(),
                version: dep_manifest.package.version.clone(),
                source,
                dependencies: child_deps.keys().cloned().collect(),
            },
        );
        for (child_name, child_dep) in child_deps {
            self.queue_dependency(&child_name, &child_dep, &dir, &dep_manifest)?;
        }
        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        loop {
            let mut changed = false;
            let names: Vec<String> = self.requirements.keys().cloned().collect();

            for name in names {
                if self.resolved.contains_key(&name) {
                    // Already pinned by a path/git dependency; registry requirements on the same
                    // name are intentionally overridden (documented pinning behavior).
                    continue;
                }

                let reqs = self.requirements.get(&name).cloned().unwrap_or_default();
                let Some((_, url)) = reqs.first() else {
                    continue;
                };
                let url = url.clone();

                let cache_key = (url.clone(), name.clone());
                let entries = match self.index_cache.get(&cache_key) {
                    Some(e) => e.clone(),
                    None => {
                        let client = open_registry(&url);
                        let entries = client.fetch_index(&name).with_context(|| {
                            format!("fetching index for '{}' from {}", name, url)
                        })?;
                        self.index_cache.insert(cache_key, entries.clone());
                        entries
                    }
                };

                if entries.is_empty() {
                    bail!("no package named '{}' found in registry {}", name, url);
                }

                let mut candidates: Vec<&IndexEntry> = entries
                    .iter()
                    .filter(|e| {
                        Version::parse(&e.vers)
                            .map(|v| reqs.iter().all(|(r, _)| r.matches(&v)))
                            .unwrap_or(false)
                    })
                    .collect();
                candidates.sort_by_key(|e| Version::parse(&e.vers).unwrap());
                let preferred_match = self
                    .preferred
                    .get(&name)
                    .and_then(|v| candidates.iter().find(|e| &e.vers == v))
                    .copied();
                let Some(chosen) = preferred_match.or_else(|| candidates.last().copied()) else {
                    let reqs_str = reqs
                        .iter()
                        .map(|(r, _)| r.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    bail!(
                        "no version of '{}' satisfies all requirements: {}",
                        name,
                        reqs_str
                    );
                };

                let already_current = self
                    .resolved
                    .get(&name)
                    .is_some_and(|p| p.version == chosen.vers);
                if already_current {
                    continue;
                }

                changed = true;
                self.resolved.insert(
                    name.clone(),
                    ResolvedPackage {
                        name: name.clone(),
                        version: chosen.vers.clone(),
                        source: ResolvedSource::Registry {
                            url: url.clone(),
                            tarball: chosen.tarball.clone(),
                            checksum: chosen.cksum.clone(),
                        },
                        dependencies: chosen.deps.iter().map(|d| d.name.clone()).collect(),
                    },
                );
                for dep in &chosen.deps {
                    let req = VersionReq::parse(&dep.req).with_context(|| {
                        format!(
                            "parsing version requirement '{}' declared by {} {} for '{}'",
                            dep.req, name, chosen.vers, dep.name
                        )
                    })?;
                    self.requirements
                        .entry(dep.name.clone())
                        .or_default()
                        .push((req, url.clone()));
                }
            }

            if !changed {
                break;
            }
        }
        Ok(())
    }
}

enum ResolvedKind {
    Path,
    Git {
        url: String,
        rev: Option<String>,
        tag: Option<String>,
        branch: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, MANIFEST_FILE_NAME};
    use crate::registry::{IndexDependency, IndexEntry};
    use std::collections::BTreeMap;

    fn publish(registry_dir: &Path, name: &str, vers: &str, deps: &[(&str, &str)]) {
        let registry =
            crate::registry::open_registry(&format!("file://{}", registry_dir.display()));
        let tarball_src = registry_dir.join(format!("staging-{}-{}.tar.gz", name, vers));
        std::fs::write(&tarball_src, b"unused in resolver tests").unwrap();
        let entry = IndexEntry {
            name: name.to_string(),
            vers: vers.to_string(),
            deps: deps
                .iter()
                .map(|(n, r)| IndexDependency {
                    name: n.to_string(),
                    req: r.to_string(),
                })
                .collect(),
            cksum: crate::registry::checksum::sha256_of(b"unused in resolver tests"),
            tarball: format!("dl/{}/{}-{}.tar.gz", name, name, vers),
            description: None,
        };
        registry.publish(&entry, &tarball_src).unwrap();
    }

    fn manifest_with_deps(deps: &[(&str, &str)], registry_url: &str) -> Manifest {
        let mut manifest = Manifest::new(
            "app".to_string(),
            "0.1.0".to_string(),
            "src/main.dream".to_string(),
        );
        manifest
            .registries
            .insert("default".to_string(), registry_url.to_string());
        for (name, req) in deps {
            manifest.dependencies.insert(
                name.to_string(),
                crate::manifest::Dependency::Version(req.to_string()),
            );
        }
        manifest
    }

    #[test]
    fn resolves_diamond_dependency_to_a_single_compatible_version() {
        let tmp = tempfile::tempdir().unwrap();
        let registry_url = format!("file://{}", tmp.path().display());

        // app -> a (^1.0), app -> b (^1.0); a -> shared (^1.0), b -> shared (^1.1) => shared 1.1.0
        publish(tmp.path(), "shared", "1.0.0", &[]);
        publish(tmp.path(), "shared", "1.1.0", &[]);
        publish(tmp.path(), "a", "1.0.0", &[("shared", "^1.0")]);
        publish(tmp.path(), "b", "1.0.0", &[("shared", "^1.1")]);

        let manifest = manifest_with_deps(&[("a", "^1.0"), ("b", "^1.0")], &registry_url);
        let resolved = resolve(&manifest, tmp.path(), false, &BTreeMap::new()).unwrap();

        let shared = resolved.iter().find(|p| p.name == "shared").unwrap();
        assert_eq!(shared.version, "1.1.0");
        assert_eq!(resolved.len(), 3);
    }

    #[test]
    fn errors_when_no_version_satisfies_every_requirement() {
        let tmp = tempfile::tempdir().unwrap();
        let registry_url = format!("file://{}", tmp.path().display());

        publish(tmp.path(), "shared", "1.0.0", &[]);
        publish(tmp.path(), "shared", "2.0.0", &[]);
        publish(tmp.path(), "a", "1.0.0", &[("shared", "^1.0")]);
        publish(tmp.path(), "b", "1.0.0", &[("shared", "^2.0")]);

        let manifest = manifest_with_deps(&[("a", "^1.0"), ("b", "^1.0")], &registry_url);
        let err = resolve(&manifest, tmp.path(), false, &BTreeMap::new()).unwrap_err();
        assert!(err.to_string().contains("no version of 'shared' satisfies"));
    }

    #[test]
    fn prefers_locked_version_when_still_compatible() {
        let tmp = tempfile::tempdir().unwrap();
        let registry_url = format!("file://{}", tmp.path().display());
        publish(tmp.path(), "pkg", "1.0.0", &[]);
        publish(tmp.path(), "pkg", "1.2.0", &[]);

        let manifest = manifest_with_deps(&[("pkg", "^1.0")], &registry_url);

        let mut preferred = BTreeMap::new();
        preferred.insert("pkg".to_string(), "1.0.0".to_string());
        let resolved = resolve(&manifest, tmp.path(), false, &preferred).unwrap();
        assert_eq!(resolved[0].version, "1.0.0");

        // Without a preference, the newest compatible version wins.
        let resolved = resolve(&manifest, tmp.path(), false, &BTreeMap::new()).unwrap();
        assert_eq!(resolved[0].version, "1.2.0");
    }

    #[test]
    fn resolves_path_dependency_from_its_own_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let lib_dir = tmp.path().join("local-lib");
        std::fs::create_dir_all(lib_dir.join("src")).unwrap();
        let lib_manifest = Manifest::new(
            "local-lib".to_string(),
            "0.2.0".to_string(),
            "src/local-lib.dream".to_string(),
        );
        lib_manifest
            .save(&lib_dir.join(MANIFEST_FILE_NAME))
            .unwrap();

        let app_dir = tmp.path().join("app");
        std::fs::create_dir_all(&app_dir).unwrap();
        let mut manifest = Manifest::new(
            "app".to_string(),
            "0.1.0".to_string(),
            "src/main.dream".to_string(),
        );
        manifest.dependencies.insert(
            "local-lib".to_string(),
            crate::manifest::Dependency::Detailed(crate::manifest::DetailedDependency {
                path: Some("../local-lib".to_string()),
                ..Default::default()
            }),
        );

        let resolved = resolve(&manifest, &app_dir, false, &BTreeMap::new()).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "local-lib");
        assert_eq!(resolved[0].version, "0.2.0");
        assert!(matches!(resolved[0].source, ResolvedSource::Path { .. }));
    }
}
