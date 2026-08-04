use crate::fetch;
use crate::registry::{checksum, open_registry, IndexDependency, IndexEntry};
use crate::workspace::Workspace;
use anyhow::{Context, Result};
use std::path::Path;

pub fn run(start_dir: &Path, registry_url: Option<String>) -> Result<()> {
    let workspace = Workspace::discover(start_dir)?;
    workspace.manifest.validate()?;

    let url = match registry_url {
        Some(u) => u,
        None => workspace
            .manifest
            .registry_url(None)
            .context("no registry configured (pass --registry or set [registries] default)")?,
    };

    let tarball_path = fetch::cache_dir().join(format!(
        "{}-{}.tar.gz",
        workspace.manifest.package.name, workspace.manifest.package.version
    ));
    let bytes = fetch::package_project(&workspace.root, &tarball_path)?;
    let cksum = checksum::sha256_of(&bytes);

    let deps: Vec<IndexDependency> = workspace
        .manifest
        .dependencies
        .iter()
        .filter_map(|(name, dep)| {
            dep.version_req()
                .map(|req| IndexDependency {
                    name: name.clone(),
                    req: req.to_string(),
                })
        })
        .collect();

    let entry = IndexEntry {
        name: workspace.manifest.package.name.clone(),
        vers: workspace.manifest.package.version.clone(),
        deps,
        cksum,
        tarball: format!(
            "dl/{}/{}-{}.tar.gz",
            workspace.manifest.package.name,
            workspace.manifest.package.name,
            workspace.manifest.package.version
        ),
        description: workspace.manifest.package.description.clone(),
    };

    let client = open_registry(&url);
    client.publish(&entry, &tarball_path)?;

    println!(
        "Published {} {} to {}",
        entry.name,
        entry.vers,
        client.base_url()
    );
    Ok(())
}
