//! Git dependency support: shells out to the system `git` binary (per the "prefer well-settled
//! tools over bespoke implementations" convention) rather than vendoring a Git implementation.

use crate::fetch::git_dir;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Clones (or reuses a cached clone of) `url` at the given `tag`/`branch`/`rev`, returning the
/// checked-out directory. Exactly one of `tag`/`branch`/`rev` is expected to be set; when none
/// are, the default branch's tip is used.
pub fn fetch_git_dependency(
    url: &str,
    tag: Option<&str>,
    branch: Option<&str>,
    rev: Option<&str>,
) -> Result<PathBuf> {
    let checkout_ref = tag.or(branch).or(rev).unwrap_or("HEAD");
    let dest = git_dir().join(format!("{}-{}", sanitize(url), sanitize(checkout_ref)));

    if dest.join(".git").is_dir() {
        return Ok(dest);
    }

    std::fs::create_dir_all(dest.parent().unwrap())?;
    let _ = std::fs::remove_dir_all(&dest);

    let mut cmd = Command::new("git");
    cmd.arg("clone").arg("--quiet");
    if let Some(t) = tag.or(branch) {
        cmd.arg("--branch").arg(t);
    }
    cmd.arg("--depth").arg("1").arg(url).arg(&dest);

    let status = cmd
        .status()
        .with_context(|| format!("running `git clone {}`", url))?;
    if !status.success() {
        bail!("`git clone {}` failed (exit code {:?})", url, status.code());
    }

    if let Some(r) = rev {
        let status = Command::new("git")
            .arg("-C")
            .arg(&dest)
            .arg("checkout")
            .arg("--quiet")
            .arg(r)
            .status()
            .with_context(|| format!("running `git checkout {}` in {}", r, dest.display()))?;
        if !status.success() {
            bail!("`git checkout {}` failed for {}", r, url);
        }
    }

    Ok(dest)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
