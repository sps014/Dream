use crate::workspace::Workspace;
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

pub fn run(start_dir: &Path, release: bool) -> Result<()> {
    super::install::run(start_dir)?;
    let workspace = Workspace::discover(start_dir)?;
    let dream_bin = crate::dream_bin::locate()?;

    let mut cmd = Command::new(&dream_bin);
    if release {
        cmd.arg("--release");
    }
    cmd.arg(workspace.entry_path());

    let status = cmd
        .status()
        .map_err(|e| anyhow::anyhow!("running {}: {}", dream_bin.display(), e))?;
    if !status.success() {
        bail!("build failed (exit code {:?})", status.code());
    }
    Ok(())
}
