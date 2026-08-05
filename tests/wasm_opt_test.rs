//! Exercises wasm-opt post-processing end-to-end: compiling with [`Compiler::with_optimize`] must
//! still produce a valid, loadable, *runnable* `.wasm` module at every level, and the optimized
//! artifact must not grow relative to a release module *without* Binaryen post-processing.
//!
//! This loops over every [`OptLevel`] (not just one) and actually instantiates + calls `main`
//! (not just `Module::validate`s the bytes): an earlier version of `optimize_wasm_file` enabled
//! Binaryen's full feature baseline, which validated fine but let `-Oz` specifically emit typed
//! function-references instructions that this project's `wasmtime::Config` doesn't opt into,
//! producing a `.wasm` that failed to even parse at runtime despite "validating". Only running
//! every level here would have caught that.

use dream::driver::compiler::{Compiler, Target};
use dream::driver::wasm_opt::OptLevel;
use dream::execution::wasm_runner::link_host_functions;
use std::fs;
use std::path::PathBuf;

const ALL_LEVELS: [OptLevel; 7] = [
    OptLevel::O0,
    OptLevel::O1,
    OptLevel::O2,
    OptLevel::O3,
    OptLevel::O4,
    OptLevel::Size,
    OptLevel::SizeAggressive,
];

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

/// Instantiates `bytes` under the same `wasmtime::Config`/host bindings the CLI's `run` command
/// uses (`src/execution/wasm_runner.rs`) and calls `main`, so a module that merely "validates" but
/// can't actually be loaded/executed under this project's runtime config still fails the test.
fn run_wasm_binary(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let config = dream::execution::host::threaded_wasm_config();
    let engine = wasmtime::Engine::new(&config)?;
    let module = wasmtime::Module::new(&engine, bytes)?;
    let shared_mem = dream::execution::host::shared_memory_for(&engine, &module)?;
    let mut store = wasmtime::Store::new(&engine, ());
    let mut linker = wasmtime::Linker::new(&engine);
    link_host_functions(&mut linker)?;
    linker.define(&mut store, "env", "memory", shared_mem.clone())?;
    linker.define_unknown_imports_as_traps(&module)?;
    let instance = linker.instantiate(&mut store, &module)?;
    if let Ok(main_func) = instance.get_typed_func::<(), ()>(&mut store, "main") {
        main_func.call(&mut store, ())?;
    }
    Ok(())
}

#[test]
fn optimized_wasm_runs_and_is_not_larger_at_every_level() {
    let dream_file = "tests/cases/collection_literals.dream".to_string();

    let plain_wat = unique_temp_path("plain", "wat");
    let plain_wasm = plain_wat.with_extension("wasm");
    let plain_wat_str = plain_wat.to_str().unwrap().to_string();

    // Release trimming without Binaryen — size baseline for each `-O` level below.
    Compiler::new(Target::Wasm)
        .with_release(true)
        .with_optimize(None)
        .compile(&dream_file, &plain_wat_str)
        .expect("unoptimized compile should succeed");
    let plain_bytes = fs::read(&plain_wasm).expect("unoptimized .wasm should exist");
    run_wasm_binary(&plain_bytes).expect("unoptimized module should execute");

    let mut cleanup = vec![
        plain_wat.clone(),
        plain_wasm,
        plain_wat.with_extension("abi.json"),
    ];

    for level in ALL_LEVELS {
        let opt_wat = unique_temp_path(&format!("optimized_{:?}", level), "wat");
        let opt_wasm = opt_wat.with_extension("wasm");
        let opt_wat_str = opt_wat.to_str().unwrap().to_string();

        Compiler::new(Target::Wasm)
            .with_release(true)
            .with_optimize(Some(level))
            .compile(&dream_file, &opt_wat_str)
            .unwrap_or_else(|e| panic!("optimized compile at {:?} should succeed: {:?}", level, e));

        let opt_bytes = fs::read(&opt_wasm)
            .unwrap_or_else(|_| panic!("optimized .wasm at {:?} should exist", level));

        assert!(
            !opt_bytes.is_empty(),
            "optimized .wasm at {:?} is empty",
            level
        );
        assert!(
            opt_bytes.len() <= plain_bytes.len(),
            "wasm-opt at {:?} grew the module: {} -> {} bytes",
            level,
            plain_bytes.len(),
            opt_bytes.len()
        );

        run_wasm_binary(&opt_bytes)
            .unwrap_or_else(|e| panic!("optimized module at {:?} should execute: {}", level, e));

        cleanup.push(opt_wat.clone());
        cleanup.push(opt_wasm);
        cleanup.push(opt_wat.with_extension("abi.json"));
    }

    for path in cleanup {
        let _ = fs::remove_file(&path);
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
