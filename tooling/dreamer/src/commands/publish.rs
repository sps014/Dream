use crate::fetch;
use crate::registry::{
    checksum, open_registry_with_token, IndexDependency, IndexEntry, MAX_TARBALL_BYTES,
};
use crate::workspace::Workspace;
use anyhow::{bail, Context, Result};
use std::path::Path;

pub fn run(start_dir: &Path, registry_url: Option<String>, token: Option<String>) -> Result<()> {
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
    if bytes.len() > MAX_TARBALL_BYTES {
        bail!(
            "package tarball is {} bytes; registry limit is {} bytes (10 MiB)",
            bytes.len(),
            MAX_TARBALL_BYTES
        );
    }
    let cksum = checksum::sha256_of(&bytes);

    let deps: Vec<IndexDependency> = workspace
        .manifest
        .dependencies
        .iter()
        .filter_map(|(name, dep)| {
            dep.version_req().map(|req| IndexDependency {
                name: name.clone(),
                req: req.to_string(),
            })
        })
        .collect();

    let pkg = &workspace.manifest.package;
    let entry = IndexEntry {
        name: pkg.name.clone(),
        vers: pkg.version.clone(),
        deps,
        cksum,
        tarball: format!(
            "dl/{}/{}-{}.tar.gz",
            pkg.name, pkg.name, pkg.version
        ),
        description: pkg.description.clone(),
        authors: pkg.authors.clone(),
        license: pkg.license.clone(),
        edition: pkg.edition.clone(),
        package_type: Some(pkg.package_type.as_str().to_string()),
        targets: pkg.targets.clone(),
        readme: fetch::find_readme_name(&workspace.root),
        keywords: pkg.keywords.clone(),
    };

    let client = open_registry_with_token(&url, token);
    client.publish(&entry, &tarball_path)?;

    println!(
        "Published {} {} to {}",
        entry.name,
        entry.vers,
        client.base_url()
    );
    Ok(())
}
