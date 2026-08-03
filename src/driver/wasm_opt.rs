//! Opt-in `.wasm` post-processing via Binaryen's `wasm-opt` (the `wasm-opt` crate), driven by the
//! CLI's `-O`/`--optimize` flag. This runs *after* the MIR pass pipeline and structural WAT DCE
//! (`src/mir/emit/wat_dce.rs`) already applied — it is an independent, coarser-grained shrink/speed
//! pass over the assembled binary, not a replacement for either.

use std::path::Path;
use std::str::FromStr;

/// Optimization preset requested via `-O`/`--optimize=<LEVEL>`, mirroring `wasm-opt`'s own CLI
/// levels (`0`-`4`, `s`, `z`) so users can carry over familiar Binaryen knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
    O4,
    /// `-Os`: optimize for size.
    Size,
    /// `-Oz`: optimize aggressively for size.
    SizeAggressive,
}

impl FromStr for OptLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "0" => Ok(OptLevel::O0),
            "1" => Ok(OptLevel::O1),
            "2" => Ok(OptLevel::O2),
            "3" => Ok(OptLevel::O3),
            "4" => Ok(OptLevel::O4),
            "s" | "S" => Ok(OptLevel::Size),
            "z" | "Z" => Ok(OptLevel::SizeAggressive),
            other => Err(format!(
                "invalid optimization level '{}' (expected one of: 0, 1, 2, 3, 4, s, z)",
                other
            )),
        }
    }
}

/// Runs Binaryen's `wasm-opt` over the `.wasm` file at `path` in place, at the given [`OptLevel`].
#[cfg(feature = "wasm-opt")]
pub fn optimize_wasm_file(path: &Path, level: OptLevel) -> Result<(), String> {
    use wasm_opt::{FeatureBaseline, OptimizationOptions};

    let mut options = match level {
        OptLevel::O0 => OptimizationOptions::new_opt_level_0(),
        OptLevel::O1 => OptimizationOptions::new_opt_level_1(),
        OptLevel::O2 => OptimizationOptions::new_opt_level_2(),
        OptLevel::O3 => OptimizationOptions::new_opt_level_3(),
        OptLevel::O4 => OptimizationOptions::new_opt_level_4(),
        OptLevel::Size => OptimizationOptions::new_optimize_for_size(),
        OptLevel::SizeAggressive => OptimizationOptions::new_optimize_for_size_aggressively(),
    };

    // Codegen unconditionally emits bulk-memory ops (`memory.fill`/`memory.copy`, see
    // `src/mir/emit/emitter/`) and other post-MVP instructions, matching what `wasmtime`'s default
    // `Config` already accepts at runtime (`src/execution/wasm_runner.rs`). `wasm-opt`'s default
    // feature baseline is narrower than that and mis-validates them as errors, so opt in to the
    // full feature set here to match the rest of the pipeline instead of guessing which subset of
    // proposals codegen currently happens to use.
    options.features.baseline = FeatureBaseline::All;

    options
        .run(path, path)
        .map_err(|e| format!("wasm-opt failed: {}", e))
}

/// Stub used when the compiler was built without the `wasm-opt` feature, so `-O`/`--optimize` still
/// fails with a clear, actionable message instead of silently doing nothing or not compiling.
#[cfg(not(feature = "wasm-opt"))]
pub fn optimize_wasm_file(_path: &Path, _level: OptLevel) -> Result<(), String> {
    Err("this build of the compiler was built without the `wasm-opt` feature; rebuild with \
         `--features wasm-opt` (enabled by default) to use -O/--optimize"
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_known_levels() {
        assert_eq!("0".parse::<OptLevel>(), Ok(OptLevel::O0));
        assert_eq!("1".parse::<OptLevel>(), Ok(OptLevel::O1));
        assert_eq!("2".parse::<OptLevel>(), Ok(OptLevel::O2));
        assert_eq!("3".parse::<OptLevel>(), Ok(OptLevel::O3));
        assert_eq!("4".parse::<OptLevel>(), Ok(OptLevel::O4));
        assert_eq!("s".parse::<OptLevel>(), Ok(OptLevel::Size));
        assert_eq!("z".parse::<OptLevel>(), Ok(OptLevel::SizeAggressive));
    }

    #[test]
    fn rejects_unknown_level() {
        assert!("bogus".parse::<OptLevel>().is_err());
    }
}
