use crate::workspace::Workspace;
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

pub fn run(start_dir: &Path, extra_args: &[String]) -> Result<()> {
    super::install::run(start_dir)?;
    let workspace = Workspace::discover(start_dir)?;
    let dream_bin = crate::dream_bin::locate()?;

    let status = Command::new(&dream_bin)
        .arg("run")
        .arg(workspace.entry_path())
        .args(extra_args)
        .status()
        .map_err(|e| anyhow::anyhow!("running {}: {}", dream_bin.display(), e))?;
    if !status.success() {
        bail!("program exited with a failure (exit code {:?})", status.code());
    }
    Ok(())
}
