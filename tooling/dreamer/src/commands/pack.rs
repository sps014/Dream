//! `dreamer pack`: release-compile a bin package and embed the `.wasm` in `dream-runner`.

use crate::manifest::PackageType;
use crate::workspace::Workspace;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supported pack triples (Dream name → rustc target).
const PACK_TRIPLES: &[(&str, &str)] = &[
    ("linux-x64", "x86_64-unknown-linux-gnu"),
    ("linux-arm64", "aarch64-unknown-linux-gnu"),
    ("macos-x64", "x86_64-apple-darwin"),
    ("macos-arm64", "aarch64-apple-darwin"),
    ("windows-x64", "x86_64-pc-windows-msvc"),
    ("windows-arm64", "aarch64-pc-windows-msvc"),
];

pub fn run(start_dir: &Path, target_args: &[String]) -> Result<()> {
    super::install::run(start_dir)?;
    let workspace = Workspace::discover(start_dir)?;
    if workspace.manifest.package.package_type == PackageType::Lib {
        bail!(
            "package '{}' is type = \"lib\" and cannot be packed (only bin packages produce \
             native executables)",
            workspace.manifest.package.name
        );
    }

    let triples = resolve_pack_targets(target_args)?;
    super::build::compile_entry(&workspace, true, None)?;

    let wasm_path = artifact_wasm_path(&workspace)?;
    if !wasm_path.is_file() {
        bail!(
            "expected release wasm at {} after build; was the entry compiled?",
            wasm_path.display()
        );
    }

    let dream_root = find_dream_workspace_root()
        .context("could not locate the Dream workspace (need tooling/dream-runner). \
                  Set DREAM_REPO to the Dream checkout root, or run pack from a tree that \
                  includes the compiler workspace")?;

    let pack_dir = workspace.root.join("target").join("pack");
    std::fs::create_dir_all(&pack_dir)
        .with_context(|| format!("creating {}", pack_dir.display()))?;

    let pkg_name = &workspace.manifest.package.name;
    for (dream_triple, rust_triple) in &triples {
        let out_name = if dream_triple.starts_with("windows-") {
            format!("{pkg_name}-{dream_triple}.exe")
        } else {
            format!("{pkg_name}-{dream_triple}")
        };
        let dest = pack_dir.join(&out_name);
        build_runner(&dream_root, &wasm_path, rust_triple, &dest)?;
        println!("packed {}", dest.display());
    }
    Ok(())
}

fn resolve_pack_targets(args: &[String]) -> Result<Vec<(String, String)>> {
    if args.is_empty() {
        let host = host_pack_triple()?;
        let rust = rust_triple_for(&host)
            .ok_or_else(|| anyhow::anyhow!("internal: unknown host pack triple {host}"))?;
        return Ok(vec![(host, rust.to_string())]);
    }

    let mut out = Vec::new();
    for arg in args {
        if arg == "all" {
            for &(d, r) in PACK_TRIPLES {
                if !out.iter().any(|(x, _)| x == d) {
                    out.push((d.to_string(), r.to_string()));
                }
            }
            continue;
        }
        let rust = rust_triple_for(arg).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown pack target '{}'; expected one of {} or 'all'",
                arg,
                PACK_TRIPLES
                    .iter()
                    .map(|(d, _)| *d)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        if !out.iter().any(|(x, _)| x == arg) {
            out.push((arg.clone(), rust.to_string()));
        }
    }
    Ok(out)
}

fn rust_triple_for(dream_triple: &str) -> Option<&'static str> {
    PACK_TRIPLES
        .iter()
        .find(|(d, _)| *d == dream_triple)
        .map(|(_, r)| *r)
}

fn host_pack_triple() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let dream_os = match os {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => bail!("unsupported host OS for pack: {other}"),
    };
    let dream_arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => bail!("unsupported host arch for pack: {other}"),
    };
    Ok(format!("{dream_os}-{dream_arch}"))
}

fn artifact_wasm_path(workspace: &Workspace) -> Result<PathBuf> {
    let entry = workspace.compile_root_path()?;
    let stem = entry
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("entry has no file stem"))?;
    Ok(workspace
        .root
        .join("target")
        .join("release")
        .join(format!("{stem}.wasm")))
}

fn find_dream_workspace_root() -> Option<PathBuf> {
    if let Ok(repo) = std::env::var("DREAM_REPO") {
        let p = PathBuf::from(repo);
        if p.join("tooling").join("dream-runner").join("Cargo.toml").is_file() {
            return Some(p);
        }
    }

    // Walk from the dream binary (or cwd) upward looking for the workspace Cargo.toml that lists
    // dream-runner.
    let mut starts = Vec::new();
    if let Ok(bin) = crate::dream_bin::locate() {
        if let Some(parent) = bin.parent() {
            starts.push(parent.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            starts.push(parent.to_path_buf());
        }
    }

    for start in starts {
        let mut dir = Some(start);
        while let Some(d) = dir {
            let candidate = d.join("tooling").join("dream-runner").join("Cargo.toml");
            if candidate.is_file() {
                return Some(d);
            }
            // Also accept being inside tooling/dreamer/target/...
            if d.join("Cargo.toml").is_file()
                && d.join("tooling").join("dream-runner").join("Cargo.toml").is_file()
            {
                return Some(d);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    None
}

fn build_runner(
    dream_root: &Path,
    wasm_path: &Path,
    rust_triple: &str,
    dest: &Path,
) -> Result<()> {
    let host_triple = host_rustc_triple()?;
    let mut cmd = Command::new("cargo");
    cmd.current_dir(dream_root)
        .env("DREAM_EMBEDDED_WASM", wasm_path)
        .args([
            "build",
            "-p",
            "dream-runner",
            "--release",
            "--manifest-path",
        ])
        .arg(dream_root.join("Cargo.toml"));

    if rust_triple != host_triple {
        cmd.arg("--target").arg(rust_triple);
    }

    let status = cmd
        .status()
        .map_err(|e| anyhow::anyhow!("running cargo build -p dream-runner: {e}"))?;
    if !status.success() {
        bail!(
            "failed to build dream-runner for {rust_triple} (exit {:?}). \
             Install the target with `rustup target add {rust_triple}` and ensure a linker \
             is available for cross-compilation",
            status.code()
        );
    }

    let bin_name = if rust_triple.contains("windows") {
        "dream-runner.exe"
    } else {
        "dream-runner"
    };
    let built = if rust_triple == host_triple {
        dream_root.join("target").join("release").join(bin_name)
    } else {
        dream_root
            .join("target")
            .join(rust_triple)
            .join("release")
            .join(bin_name)
    };
    if !built.is_file() {
        bail!("cargo reported success but {} is missing", built.display());
    }
    std::fs::copy(&built, dest)
        .with_context(|| format!("copying {} → {}", built.display(), dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dest, perms)?;
    }
    Ok(())
}

fn host_rustc_triple() -> Result<String> {
    // Prefer rustc's host so we match cargo's default target directory layout.
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("running rustc -vV")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(host) = line.strip_prefix("host: ") {
            return Ok(host.trim().to_string());
        }
    }
    bail!("could not parse host triple from rustc -vV");
}
