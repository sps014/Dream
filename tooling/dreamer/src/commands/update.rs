use crate::resolver;
use crate::workspace::Workspace;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Re-resolves ignoring the existing lock's pinned versions (except for `name`, when given, which
/// updates just that one package and keeps every other pin as-is).
pub fn run(start_dir: &Path, name: Option<String>) -> Result<()> {
    let workspace = Workspace::discover(start_dir)?;

    let preferred: BTreeMap<String, String> = match (
        &name,
        crate::lockfile::Lockfile::load_if_exists(&workspace.lockfile_path())?,
    ) {
        (Some(keep_others_pinned), Some(lock)) => lock
            .packages
            .into_iter()
            .filter(|p| &p.name != keep_others_pinned)
            .map(|p| (p.name, p.version))
            .collect(),
        _ => BTreeMap::new(),
    };

    let resolved = resolver::resolve(&workspace.manifest, &workspace.root, true, &preferred)?;
    let lockfile = workspace.install(&resolved)?;
    lockfile.save(&workspace.lockfile_path())?;

    match name {
        Some(name) => println!(
            "Updated '{}' (and re-resolved anything affected by it)",
            name
        ),
        None => println!("Updated all dependencies to the latest compatible versions"),
    }
    for pkg in &lockfile.packages {
        println!("  {} {}", pkg.name, pkg.version);
    }
    Ok(())
}
