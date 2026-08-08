use crate::manifest::{resolve_run_target, PackageType, RunTarget};
use crate::workspace::Workspace;
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

pub fn run(
    start_dir: &Path,
    target: Option<String>,
    release: bool,
    port: Option<u16>,
    extra_args: &[String],
) -> Result<()> {
    super::install::run(start_dir)?;
    let workspace = Workspace::discover(start_dir)?;
    if workspace.manifest.package.package_type == PackageType::Lib {
        bail!(
            "package '{}' is type = \"lib\" and is not runnable (use dreamer build to typecheck)",
            workspace.manifest.package.name
        );
    }
    let host = resolve_run_target(&workspace.manifest.package.targets, target.as_deref())?;

    match host {
        RunTarget::Native => run_native(&workspace, release, extra_args),
        RunTarget::Node => run_node(&workspace, release, extra_args),
        RunTarget::Web => run_web(&workspace, release, port),
    }
}

fn run_native(workspace: &Workspace, release: bool, extra_args: &[String]) -> Result<()> {
    let dream_bin = crate::dream_bin::locate()?;
    let entry = workspace.compile_root_path()?;
    let mut cmd = Command::new(&dream_bin);
    cmd.arg("run");
    if release {
        cmd.arg("--release");
    }
    cmd.arg("--crate-type").arg("bin").arg(&entry).args(extra_args);
    let status = cmd
        .status()
        .map_err(|e| anyhow::anyhow!("running {}: {}", dream_bin.display(), e))?;
    if !status.success() {
        bail!(
            "program exited with a failure (exit code {:?})",
            status.code()
        );
    }
    Ok(())
}

fn run_node(workspace: &Workspace, release: bool, extra_args: &[String]) -> Result<()> {
    let run_mjs = workspace.root.join("run.mjs");
    if !run_mjs.is_file() {
        bail!(
            "missing {}; re-run `dreamer init --runtime node` or add a Node runner that imports \
             the entry's *.node.runtime.js from target/node/",
            run_mjs.display()
        );
    }

    super::build::compile_entry(workspace, release, Some(RunTarget::Node))?;

    let status = Command::new("node")
        .arg(&run_mjs)
        .args(extra_args)
        .current_dir(&workspace.root)
        .status()
        .map_err(|e| anyhow::anyhow!("running node: {}", e))?;
    if !status.success() {
        bail!(
            "node exited with a failure (exit code {:?})",
            status.code()
        );
    }
    Ok(())
}

fn run_web(workspace: &Workspace, release: bool, port: Option<u16>) -> Result<()> {
    let index = workspace.root.join("index.html");
    if !index.is_file() {
        bail!(
            "missing {}; re-run `dreamer init --runtime web` or add an index.html that imports \
             the entry's *.web.runtime.js from target/web/",
            index.display()
        );
    }

    super::build::compile_entry(workspace, release, Some(RunTarget::Web))?;
    let port = port.unwrap_or(crate::serve::DEFAULT_WEB_PORT);
    crate::serve::serve_project(&workspace.root, port)
}
