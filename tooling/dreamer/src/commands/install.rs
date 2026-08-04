use crate::lockfile::Lockfile;
use crate::resolver;
use crate::workspace::Workspace;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

pub fn run(start_dir: &Path) -> Result<()> {
    let workspace = Workspace::discover(start_dir)?;

    let existing_lock = Lockfile::load_if_exists(&workspace.lockfile_path())?;
    let preferred: BTreeMap<String, String> = existing_lock
        .map(|lock| {
            lock.packages
                .into_iter()
                .map(|p| (p.name, p.version))
                .collect()
        })
        .unwrap_or_default();

    let resolved = resolver::resolve(&workspace.manifest, &workspace.root, true, &preferred)?;
    let lockfile = workspace.install(&resolved)?;
    lockfile.save(&workspace.lockfile_path())?;

    println!(
        "Installed {} package(s) into {}",
        lockfile.packages.len(),
        workspace.packages_dir().display()
    );
    for pkg in &lockfile.packages {
        println!("  {} {}", pkg.name, pkg.version);
    }
    Ok(())
}
