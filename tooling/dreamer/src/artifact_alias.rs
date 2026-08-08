//! Stable host artifact aliases under `target/web/` and `target/node/`.
//!
//! Real compiler output stays in `target/debug|release/`. After a build that emits JS runtimes,
//! dreamer copies the relevant siblings into these alias dirs so `index.html` / `run.mjs` can
//! reference a profile-independent path.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

/// Refresh `target/web` and/or `target/node` from `target/{debug|release}` after a compile.
pub fn refresh_host_aliases(
    project_root: &Path,
    entry_stem: &str,
    release: bool,
    web: bool,
    node: bool,
) -> Result<()> {
    if !web && !node {
        return Ok(());
    }

    let profile = if release { "release" } else { "debug" };
    let src_dir = project_root.join("target").join(profile);
    if !src_dir.is_dir() {
        bail!(
            "expected compile artifacts under {} after build",
            src_dir.display()
        );
    }

    if web {
        refresh_one(
            &src_dir,
            &project_root.join("target").join("web"),
            entry_stem,
            HostAlias::Web,
        )?;
    }
    if node {
        refresh_one(
            &src_dir,
            &project_root.join("target").join("node"),
            entry_stem,
            HostAlias::Node,
        )?;
    }
    Ok(())
}

enum HostAlias {
    Web,
    Node,
}

fn refresh_one(src_dir: &Path, dest_dir: &Path, stem: &str, host: HostAlias) -> Result<()> {
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating alias dir {}", dest_dir.display()))?;

    let runtime_name = match host {
        HostAlias::Web => format!("{stem}.web.runtime.js"),
        HostAlias::Node => format!("{stem}.node.runtime.js"),
    };

    copy_required(src_dir, dest_dir, &format!("{stem}.wasm"))?;
    copy_required(src_dir, dest_dir, &runtime_name)?;
    // Optional sidecar used by some hosts / tooling.
    let _ = copy_if_present(src_dir, dest_dir, &format!("{stem}.abi.json"));
    Ok(())
}

fn copy_required(src_dir: &Path, dest_dir: &Path, name: &str) -> Result<()> {
    let src = src_dir.join(name);
    if !src.is_file() {
        bail!(
            "missing {} after build (needed for host alias {})",
            src.display(),
            dest_dir.display()
        );
    }
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest)
        .with_context(|| format!("copying {} → {}", src.display(), dest.display()))?;
    Ok(())
}

fn copy_if_present(src_dir: &Path, dest_dir: &Path, name: &str) -> Result<bool> {
    let src = src_dir.join(name);
    if !src.is_file() {
        return Ok(false);
    }
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest)
        .with_context(|| format!("copying {} → {}", src.display(), dest.display()))?;
    Ok(true)
}

/// File stem of the package compile root (`src/main.dream` → `main`).
pub fn entry_stem(compile_root: &Path) -> Result<String> {
    compile_root
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("compile root has no file stem: {}", compile_root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_copies_web_and_node_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let debug = root.join("target").join("debug");
        fs::create_dir_all(&debug).unwrap();
        fs::write(debug.join("main.wasm"), b"wasm").unwrap();
        fs::write(debug.join("main.web.runtime.js"), b"web").unwrap();
        fs::write(debug.join("main.node.runtime.js"), b"node").unwrap();
        fs::write(debug.join("main.abi.json"), b"{}").unwrap();

        refresh_host_aliases(root, "main", false, true, true).unwrap();

        assert_eq!(
            fs::read(root.join("target/web/main.wasm")).unwrap(),
            b"wasm"
        );
        assert_eq!(
            fs::read(root.join("target/web/main.web.runtime.js")).unwrap(),
            b"web"
        );
        assert_eq!(
            fs::read(root.join("target/web/main.abi.json")).unwrap(),
            b"{}"
        );
        assert_eq!(
            fs::read(root.join("target/node/main.node.runtime.js")).unwrap(),
            b"node"
        );
    }

    #[test]
    fn refresh_release_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let release = root.join("target").join("release");
        fs::create_dir_all(&release).unwrap();
        fs::write(release.join("app.wasm"), b"r").unwrap();
        fs::write(release.join("app.web.runtime.js"), b"w").unwrap();

        refresh_host_aliases(root, "app", true, true, false).unwrap();
        assert!(root.join("target/web/app.wasm").is_file());
        assert!(!root.join("target/node").exists());
    }
}
