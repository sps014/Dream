use crate::artifact_alias;
use crate::manifest::{PackageType, RunTarget};
use crate::workspace::Workspace;
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

pub fn run(start_dir: &Path, release: bool) -> Result<()> {
    super::install::run(start_dir)?;
    let workspace = Workspace::discover(start_dir)?;
    compile_entry(&workspace, release, None)
}

/// Compile the workspace root, optionally restricting JS runtime emission to `only`
/// (used by `dreamer run` for a single selected host). When `only` is `None`, emit every
/// JS host listed in `package.targets`.
///
/// After a successful compile that emitted web and/or node runtimes, refreshes
/// `target/web/` and/or `target/node/` aliases from the active profile.
pub fn compile_entry(
    workspace: &Workspace,
    release: bool,
    only: Option<RunTarget>,
) -> Result<()> {
    let dream_bin = crate::dream_bin::locate()?;
    let compile_root = workspace.compile_root_path()?;

    let mut cmd = Command::new(&dream_bin);
    if release {
        cmd.arg("--release");
    }

    match workspace.manifest.package.package_type {
        PackageType::Lib => {
            cmd.arg("--crate-type");
            cmd.arg("lib");
        }
        PackageType::Bin => {
            cmd.arg("--crate-type");
            cmd.arg("bin");
        }
    }

    let want_web = match only {
        Some(RunTarget::Web) => true,
        Some(_) => false,
        None => workspace
            .manifest
            .package
            .targets
            .iter()
            .any(|t| t == "web"),
    };
    let want_node = match only {
        Some(RunTarget::Node) => true,
        Some(_) => false,
        None => workspace
            .manifest
            .package
            .targets
            .iter()
            .any(|t| t == "node"),
    };

    if want_web || want_node {
        cmd.arg("--runtime");
        if want_web {
            cmd.arg("--web");
        }
        if want_node {
            cmd.arg("--node");
        }
    }

    cmd.arg(&compile_root);

    let status = cmd
        .status()
        .map_err(|e| anyhow::anyhow!("running {}: {}", dream_bin.display(), e))?;
    if !status.success() {
        bail!("build failed (exit code {:?})", status.code());
    }

    if want_web || want_node {
        let stem = artifact_alias::entry_stem(&compile_root)?;
        artifact_alias::refresh_host_aliases(
            &workspace.root,
            &stem,
            release,
            want_web,
            want_node,
        )?;
    }
    Ok(())
}
