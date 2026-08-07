use crate::manifest::{resolve_run_target, RunTarget};
use crate::workspace::Workspace;
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

pub fn run(start_dir: &Path, target: Option<String>, extra_args: &[String]) -> Result<()> {
    super::install::run(start_dir)?;
    let workspace = Workspace::discover(start_dir)?;
    let host = resolve_run_target(&workspace.manifest.package.targets, target.as_deref())?;

    match host {
        RunTarget::Native => run_native(&workspace, extra_args),
        RunTarget::Node => run_node(&workspace, extra_args),
        RunTarget::Web => run_web(&workspace),
    }
}

fn run_native(workspace: &Workspace, extra_args: &[String]) -> Result<()> {
    let dream_bin = crate::dream_bin::locate()?;
    let status = Command::new(&dream_bin)
        .arg("run")
        .arg(workspace.entry_path())
        .args(extra_args)
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

fn run_node(workspace: &Workspace, extra_args: &[String]) -> Result<()> {
    let run_mjs = workspace.root.join("run.mjs");
    if !run_mjs.is_file() {
        bail!(
            "missing {}; re-run `dreamer init --runtime node` or add a Node runner that imports \
             the entry's *.node.runtime.js",
            run_mjs.display()
        );
    }

    super::build::compile_entry(workspace, false, Some(RunTarget::Node))?;

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

fn run_web(workspace: &Workspace) -> Result<()> {
    let index = workspace.root.join("index.html");
    if !index.is_file() {
        bail!(
            "missing {}; re-run `dreamer init --runtime web` or add an index.html that imports \
             the entry's *.web.runtime.js",
            index.display()
        );
    }

    super::build::compile_entry(workspace, false, Some(RunTarget::Web))?;
    crate::serve::serve_project(&workspace.root)
}
