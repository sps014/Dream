use crate::workspace::Workspace;
use anyhow::{bail, Result};
use std::path::Path;

pub fn run(start_dir: &Path, name: &str) -> Result<()> {
    let mut workspace = Workspace::discover(start_dir)?;

    let removed_dep = workspace.manifest.dependencies.remove(name).is_some();
    let removed_dev = workspace.manifest.dev_dependencies.remove(name).is_some();
    if !removed_dep && !removed_dev {
        bail!("'{}' is not a dependency of {}", name, workspace.manifest.package.name);
    }
    workspace.save_manifest()?;

    let dest = workspace
        .packages_dir()
        .join(crate::manifest::import_segment(name));
    if dest.is_symlink() {
        std::fs::remove_file(&dest)?;
    } else if dest.is_dir() {
        std::fs::remove_dir_all(&dest)?;
    }

    println!("Removed '{}'", name);
    super::install::run(start_dir)
}
