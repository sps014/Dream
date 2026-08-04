//! `dream.toml` project manifest: package metadata, dependencies, dev-dependencies, scripts, and
//! registry aliases. Parsed with `serde` + `toml`, mirroring how Cargo reads `Cargo.toml`.

use anyhow::{bail, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE_NAME: &str = "dream.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub package: PackageMeta,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(default, rename = "dev-dependencies")]
    pub dev_dependencies: BTreeMap<String, Dependency>,
    /// Named shell commands a developer can invoke via their own shell (e.g. `sh -c
    /// "$(dreamer script start)"`). Not consumed directly by any `dreamer` subcommand in this
    /// version — kept as project metadata for tooling/documentation purposes.
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    /// Named registry aliases (`name -> base URL`), referenced from `[dependencies]` entries via
    /// `registry = "name"`. The `default` alias is used when a dependency omits `registry`.
    #[serde(default)]
    pub registries: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Compiler entry point, relative to the manifest's directory (e.g. `src/main.dream`).
    pub entry: String,
    #[serde(default)]
    pub license: Option<String>,
}

/// A dependency requirement: either a bare semver requirement string (`"^1.2"`) or a detailed
/// table (`{ version = "...", path = "...", git = "..." }`), matching Cargo's `Cargo.toml` shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Version(String),
    Detailed(DetailedDependency),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DetailedDependency {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub rev: Option<String>,
}

impl Dependency {
    pub fn version_req(&self) -> Option<&str> {
        match self {
            Dependency::Version(v) => Some(v.as_str()),
            Dependency::Detailed(d) => d.version.as_deref(),
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Dependency::Version(_) => None,
            Dependency::Detailed(d) => d.path.as_deref(),
        }
    }

    pub fn git(&self) -> Option<&str> {
        match self {
            Dependency::Version(_) => None,
            Dependency::Detailed(d) => d.git.as_deref(),
        }
    }

    pub fn registry_alias(&self) -> Option<&str> {
        match self {
            Dependency::Version(_) => None,
            Dependency::Detailed(d) => d.registry.as_deref(),
        }
    }

    pub fn detailed(&self) -> DetailedDependency {
        match self {
            Dependency::Version(v) => DetailedDependency {
                version: Some(v.clone()),
                ..Default::default()
            },
            Dependency::Detailed(d) => d.clone(),
        }
    }

    pub fn to_toml_value(&self) -> toml::Value {
        match self {
            Dependency::Version(v) => toml::Value::String(v.clone()),
            Dependency::Detailed(d) => {
                let mut table = toml::value::Table::new();
                if let Some(v) = &d.version {
                    table.insert("version".into(), toml::Value::String(v.clone()));
                }
                if let Some(v) = &d.registry {
                    table.insert("registry".into(), toml::Value::String(v.clone()));
                }
                if let Some(v) = &d.path {
                    table.insert("path".into(), toml::Value::String(v.clone()));
                }
                if let Some(v) = &d.git {
                    table.insert("git".into(), toml::Value::String(v.clone()));
                }
                if let Some(v) = &d.tag {
                    table.insert("tag".into(), toml::Value::String(v.clone()));
                }
                if let Some(v) = &d.branch {
                    table.insert("branch".into(), toml::Value::String(v.clone()));
                }
                if let Some(v) = &d.rev {
                    table.insert("rev".into(), toml::Value::String(v.clone()));
                }
                toml::Value::Table(table)
            }
        }
    }
}

/// Package names are restricted to the same "hyphen or underscore, alphanumeric" shape Cargo
/// uses for crate names, since a hyphenated name maps to an underscored `import` segment
/// (`json-tools` -> `import json_tools...;`), mirroring how Rust crate names relate to `use` paths.
pub fn validate_package_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("package name must not be empty");
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let starts_alpha = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    if !valid || !starts_alpha {
        bail!(
            "invalid package name '{}': must start with a letter and contain only \
             ASCII letters, digits, '-', or '_'",
            name
        );
    }
    Ok(())
}

/// Maps a package name to the identifier used in `import` statements (`json-tools` ->
/// `json_tools`), matching how Rust crate names with hyphens are referenced via `use`.
pub fn import_segment(package_name: &str) -> String {
    package_name.replace('-', "_")
}

impl Manifest {
    pub fn new(name: String, version: String, entry: String) -> Self {
        Manifest {
            package: PackageMeta {
                name,
                version,
                edition: None,
                authors: Vec::new(),
                description: None,
                entry,
                license: None,
            },
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            scripts: BTreeMap::new(),
            registries: BTreeMap::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Manifest> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest at {}", path.display()))?;
        let manifest: Manifest = toml::from_str(&text)
            .with_context(|| format!("parsing manifest at {}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        validate_package_name(&self.package.name)?;
        Version::parse(&self.package.version).with_context(|| {
            format!(
                "package '{}' has invalid version '{}' (expected semver, e.g. '1.2.3')",
                self.package.name, self.package.version
            )
        })?;
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)
            .with_context(|| format!("serializing manifest for {}", path.display()))?;
        std::fs::write(path, text)
            .with_context(|| format!("writing manifest at {}", path.display()))?;
        Ok(())
    }

    /// Walks upward from `start_dir` looking for the nearest `dream.toml`, mirroring how Cargo
    /// discovers the enclosing project root from any subdirectory.
    pub fn find_project_root(start_dir: &Path) -> Option<PathBuf> {
        let mut dir = Some(start_dir.to_path_buf());
        while let Some(d) = dir {
            if d.join(MANIFEST_FILE_NAME).is_file() {
                return Some(d);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        None
    }

    /// All resolvable dependency entries, optionally including `[dev-dependencies]`.
    pub fn all_dependencies(&self, include_dev: bool) -> BTreeMap<String, Dependency> {
        let mut out = self.dependencies.clone();
        if include_dev {
            out.extend(self.dev_dependencies.clone());
        }
        out
    }

    /// Resolves a dependency's registry alias to a base URL, falling back to `[registries]
    /// default`, then to the built-in default registry.
    pub fn registry_url(&self, alias: Option<&str>) -> Option<String> {
        let alias = alias.unwrap_or("default");
        self.registries
            .get(alias)
            .cloned()
            .or_else(|| (alias == "default").then(|| crate::registry::DEFAULT_REGISTRY.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_package_names() {
        assert!(validate_package_name("json-tools").is_ok());
        assert!(validate_package_name("json_tools").is_ok());
        assert!(validate_package_name("a").is_ok());
        assert!(validate_package_name("").is_err());
        assert!(validate_package_name("1abc").is_err());
        assert!(validate_package_name("has space").is_err());
        assert!(validate_package_name("has.dot").is_err());
    }

    #[test]
    fn maps_hyphens_to_underscores_for_import_segments() {
        assert_eq!(import_segment("json-tools"), "json_tools");
        assert_eq!(import_segment("already_underscored"), "already_underscored");
    }

    #[test]
    fn round_trips_through_toml() {
        let mut manifest = Manifest::new(
            "myapp".to_string(),
            "0.1.0".to_string(),
            "src/main.dream".to_string(),
        );
        manifest.dependencies.insert(
            "json-tools".to_string(),
            Dependency::Version("^0.3".to_string()),
        );
        manifest.dependencies.insert(
            "local-lib".to_string(),
            Dependency::Detailed(DetailedDependency {
                path: Some("../local-lib".to_string()),
                ..Default::default()
            }),
        );

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(MANIFEST_FILE_NAME);
        manifest.save(&path).unwrap();

        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.package.name, "myapp");
        assert_eq!(loaded.dependencies.len(), 2);
        assert_eq!(
            loaded.dependencies.get("json-tools").unwrap().version_req(),
            Some("^0.3")
        );
        assert_eq!(
            loaded.dependencies.get("local-lib").unwrap().path(),
            Some("../local-lib")
        );
    }

    #[test]
    fn rejects_invalid_semver_on_validate() {
        let manifest = Manifest::new(
            "myapp".to_string(),
            "not-a-version".to_string(),
            "src/main.dream".to_string(),
        );
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn finds_project_root_by_walking_upward() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest::new(
            "myapp".to_string(),
            "0.1.0".to_string(),
            "src/main.dream".to_string(),
        );
        manifest.save(&tmp.path().join(MANIFEST_FILE_NAME)).unwrap();

        let nested = tmp.path().join("src").join("deep").join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            Manifest::find_project_root(&nested),
            Some(tmp.path().to_path_buf())
        );
    }

    #[test]
    fn no_project_root_found_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::find_project_root(tmp.path()), None);
    }
}
