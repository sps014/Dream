//! Locates the `dream` compiler executable that `dreamer build`/`dreamer run` shell out to.

use anyhow::{bail, Result};
use std::path::PathBuf;

pub fn locate() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("DREAM_BIN") {
        let path = PathBuf::from(custom);
        if path.is_file() {
            return Ok(path);
        }
    }

    if let Some(on_path) = find_on_path("dream") {
        return Ok(on_path);
    }

    // When `dreamer` itself was built inside the Dream workspace, its own binary and the `dream`
    // compiler binary land as siblings in the same `target/{debug,release}/` directory (one
    // shared target dir per Cargo workspace) — check there before falling back to walking up
    // from the current working directory, since the latter only works when *running* from
    // inside the repo (e.g. `dreamer`'s own tests, whose cwd is an unrelated temp directory).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(if cfg!(windows) { "dream.exe" } else { "dream" });
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }

    // Fall back to a locally built binary, so `dreamer` is usable while developing inside the
    // Dream monorepo itself, before `dream` has been installed anywhere on `PATH`. Debug is
    // checked first: it's rebuilt far more often during the day-to-day `cargo build`/`cargo test`
    // inner loop, so preferring it avoids silently running a stale `--release` binary that just
    // happens to exist from an earlier, unrelated build.
    for candidate in ["target/debug/dream", "target/release/dream"] {
        let mut dir = std::env::current_dir()?;
        loop {
            let path = dir.join(candidate);
            if path.is_file() {
                return Ok(path);
            }
            let Some(parent) = dir.parent() else { break };
            dir = parent.to_path_buf();
        }
    }

    bail!(
        "could not find the `dream` compiler executable; install it on PATH, set DREAM_BIN, \
         or run `cargo build --release` from the Dream repo"
    )
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(&exe_name))
        .find(|p| p.is_file())
}
