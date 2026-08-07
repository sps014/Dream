//! Locates the `dream` compiler executable that `dreamer build`/`dreamer run` shell out to.
//!
//! Resolution order (first hit wins):
//! 1. `DREAM_BIN` — exact path to the `dream` binary
//! 2. `DREAM_HOME` — directory containing `dream` / `dream.exe`
//! 3. `~/.dream/toolchain.env` written by `use-toolchain.sh` (for tools run outside that shell)
//! 4. `dream` on `PATH`
//! 5. Sibling of this `dreamer` executable (same Cargo `target/{debug,release}/`)
//! 6. Walk upward from cwd for `target/debug/dream` then `target/release/dream`

use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn read_toolchain_env_file() -> BTreeMap<String, String> {
    let Some(home) = dirs::home_dir() else {
        return BTreeMap::new();
    };
    let path = home.join(".dream").join("toolchain.env");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_string();
        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            value = value[1..value.len() - 1].to_string();
        }
        out.insert(key.trim().to_string(), value);
    }
    out
}

fn env_or_toolchain(key: &str, file: &BTreeMap<String, String>) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| file.get(key).cloned())
}

pub fn locate() -> Result<PathBuf> {
    let file = read_toolchain_env_file();

    if let Some(custom) = env_or_toolchain("DREAM_BIN", &file) {
        let path = PathBuf::from(&custom);
        if path.is_file() {
            return Ok(path);
        }
        if std::env::var_os("DREAM_BIN").is_some() {
            bail!(
                "DREAM_BIN is set to '{}' but that path is not a file",
                path.display()
            );
        }
    }

    if let Some(home) = env_or_toolchain("DREAM_HOME", &file) {
        let home = PathBuf::from(&home);
        if let Some(path) = binary_in_dir(&home, "dream") {
            return Ok(path);
        }
        if std::env::var_os("DREAM_HOME").is_some() {
            bail!(
                "DREAM_HOME is set to '{}' but no `dream` binary was found there",
                home.display()
            );
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
            if let Some(sibling) = binary_in_dir(dir, "dream") {
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
            // Windows release/debug may use .exe
            let path_exe = dir.join(format!("{}.exe", candidate));
            if path_exe.is_file() {
                return Ok(path_exe);
            }
            let Some(parent) = dir.parent() else { break };
            dir = parent.to_path_buf();
        }
    }

    bail!(
        "could not find the `dream` compiler executable; install it on PATH, set DREAM_HOME \
         (directory containing `dream`) or DREAM_BIN (exact path), run `source ./use-toolchain.sh` \
         (writes ~/.dream/toolchain.env), or `cargo build` from the Dream repo"
    )
}

/// Locates the `dreamer` package-manager binary (for tooling that shells out to it).
///
/// Resolution order: `DREAMER_HOME` → `~/.dream/toolchain.env` → `PATH` → sibling of
/// `current_exe` → cwd walk for `target/{debug,release}/dreamer`.
pub fn locate_dreamer() -> Result<PathBuf> {
    let file = read_toolchain_env_file();

    if let Some(home) = env_or_toolchain("DREAMER_HOME", &file)
        .or_else(|| env_or_toolchain("DREAM_HOME", &file))
    {
        let home = PathBuf::from(&home);
        if let Some(path) = binary_in_dir(&home, "dreamer") {
            return Ok(path);
        }
        if std::env::var_os("DREAMER_HOME").is_some() {
            bail!(
                "DREAMER_HOME is set to '{}' but no `dreamer` binary was found there",
                home.display()
            );
        }
    }

    if let Some(on_path) = find_on_path("dreamer") {
        return Ok(on_path);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(sibling) = binary_in_dir(dir, "dreamer") {
                return Ok(sibling);
            }
        }
    }

    for candidate in ["target/debug/dreamer", "target/release/dreamer"] {
        let mut dir = std::env::current_dir()?;
        loop {
            let path = dir.join(candidate);
            if path.is_file() {
                return Ok(path);
            }
            let path_exe = dir.join(format!("{}.exe", candidate));
            if path_exe.is_file() {
                return Ok(path_exe);
            }
            let Some(parent) = dir.parent() else { break };
            dir = parent.to_path_buf();
        }
    }

    bail!(
        "could not find the `dreamer` executable; install it on PATH, set DREAMER_HOME \
         (directory containing `dreamer`), run `source ./use-toolchain.sh`, or \
         `cargo build -p dreamer` from the Dream repo"
    )
}

fn binary_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        dir.join(format!("{}.exe", name))
    } else {
        dir.join(name)
    };
    exe.is_file().then_some(exe)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env mutation must be serialized across tests in this process.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn binary_in_dir_finds_named_file() {
        let tmp = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "dream.exe" } else { "dream" };
        let path = tmp.path().join(name);
        std::fs::write(&path, b"").unwrap();
        assert_eq!(binary_in_dir(tmp.path(), "dream").as_deref(), Some(path.as_path()));
    }

    #[test]
    fn locate_prefers_dream_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "dream.exe" } else { "dream" };
        let path = tmp.path().join(name);
        std::fs::write(&path, b"").unwrap();

        std::env::remove_var("DREAM_BIN");
        std::env::set_var("DREAM_HOME", tmp.path());
        let located = locate().unwrap();
        std::env::remove_var("DREAM_HOME");
        assert_eq!(located, path);
    }
}
