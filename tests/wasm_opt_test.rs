//! Exercises the opt-in `-O`/`--optimize` post-processing path end-to-end: compiling with
//! [`Compiler::with_optimize`] must still produce a valid, loadable `.wasm` module, and the
//! optimized artifact must not grow relative to the unoptimized one.

use dream::driver::compiler::{Compiler, Target};
use dream::driver::wasm_opt::OptLevel;
use std::fs;
use std::path::PathBuf;

fn unique_temp_path(name: &str, ext: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "dream_wasm_opt_test_{}_{}.{}",
        name,
        std::process::id(),
        ext
    ));
    path
}

#[test]
fn optimized_wasm_is_valid_and_not_larger() {
    let dream_file = "tests/cases/collection_literals.dream".to_string();

    let plain_wat = unique_temp_path("plain", "wat");
    let plain_wasm = plain_wat.with_extension("wasm");
    let optimized_wat = unique_temp_path("optimized", "wat");
    let optimized_wasm = optimized_wat.with_extension("wasm");

    let plain_wat_str = plain_wat.to_str().unwrap().to_string();
    let optimized_wat_str = optimized_wat.to_str().unwrap().to_string();

    Compiler::new(Target::Wasm)
        .compile(&dream_file, &plain_wat_str)
        .expect("unoptimized compile should succeed");

    Compiler::new(Target::Wasm)
        .with_optimize(Some(OptLevel::Size))
        .compile(&dream_file, &optimized_wat_str)
        .expect("optimized compile should succeed");

    let plain_bytes = fs::read(&plain_wasm).expect("unoptimized .wasm should exist");
    let optimized_bytes = fs::read(&optimized_wasm).expect("optimized .wasm should exist");

    assert!(!optimized_bytes.is_empty(), "optimized .wasm is empty");
    assert!(
        optimized_bytes.len() <= plain_bytes.len(),
        "wasm-opt -Os grew the module: {} -> {} bytes",
        plain_bytes.len(),
        optimized_bytes.len()
    );

    // The optimized module must still be a well-formed, loadable wasm binary.
    wasmtime::Module::validate(&wasmtime::Engine::default(), &optimized_bytes)
        .expect("wasm-opt output should still validate as wasm");

    for path in [
        &plain_wat,
        &plain_wasm,
        &optimized_wat,
        &optimized_wasm,
        &plain_wat.with_extension("abi.json"),
        &optimized_wat.with_extension("abi.json"),
    ] {
        let _ = fs::remove_file(path);
    }
}

#[test]
fn opt_level_parses_expected_strings() {
    use std::str::FromStr;

    assert_eq!(OptLevel::from_str("s").unwrap(), OptLevel::Size);
    assert_eq!(OptLevel::from_str("z").unwrap(), OptLevel::SizeAggressive);
    assert_eq!(OptLevel::from_str("3").unwrap(), OptLevel::O3);
    assert!(OptLevel::from_str("nope").is_err());
}
