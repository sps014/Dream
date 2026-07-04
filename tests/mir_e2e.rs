//! End-to-end coverage gate for the MIR backend.
//!
//! Compiles every `tests/cases/*.dream` through the **real driver** (prelude, json-derive, multi-file
//! resolution, analysis, and the HIR → MIR → WAT backend), runs the result under `wasmtime`, and
//! compares to the `.expected` output.
//!
//! The assertion is a ratchet: every case **not** in `XFAIL` must pass, and `XFAIL` is currently
//! empty (the backend covers the whole test corpus). Any regression that breaks a previously-passing
//! case fails the suite.

use dream::driver::compiler::{Compiler, Target};
use dream::execution::host::{
    link_console_functions, link_datetime_functions, link_file_functions, link_http_functions,
    link_math_functions, link_regex_functions, link_worker_functions, read_string_from_memory,
    set_worker_module,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use wasmtime::*;

// Every case in `tests/cases` now compiles and runs through the MIR backend, so `XFAIL` is empty.
// Keep it (rather than deleting the machinery) so a future regression re-adds an entry here with a
// reason instead of silently flipping the ratchet.
const XFAIL: &[(&str, &str)] = &[];

#[derive(Clone)]
struct TestEnv {
    output: Arc<Mutex<String>>,
}

impl TestEnv {
    fn new() -> Self {
        Self {
            output: Arc::new(Mutex::new(String::new())),
        }
    }
    fn print(&self, s: &str) {
        self.output.lock().unwrap().push_str(s);
    }
}

/// Compiles one case through the MIR backend and runs it, returning `Ok(actual_output)` or an error
/// describing the failure stage (compile / assemble / instantiate / execute).
fn compile_and_run_mir(dream_file: &Path) -> Result<String, String> {
    let wat_path = dream_file.with_extension("mir.wat");
    let dream_str = dream_file.to_str().unwrap().to_string();
    let wat_str = wat_path.to_str().unwrap().to_string();

    let compiler = Compiler::new(Target::Wasm).with_debug(true);
    compiler
        .compile(&dream_str, &wat_str)
        .map_err(|e| format!("compile: {e:?}"))?;

    let wat = fs::read_to_string(&wat_path).map_err(|e| format!("read wat: {e}"))?;
    let _ = fs::remove_file(&wat_path);
    let _ = fs::remove_file(wat_path.with_extension("wasm"));
    let _ = fs::remove_file(wat_path.with_extension("abi.json"));

    let wasm = wat::parse_str(&wat).map_err(|e| format!("assemble: {e}"))?;
    // Make the module bytes available to `WebWorker` spawns on this thread.
    set_worker_module(&wasm);
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm).map_err(|e| format!("module: {e:#}"))?;
    let mut store = Store::new(&engine, ());
    let mut linker = Linker::new(&engine);
    let env = TestEnv::new();

    let e = env.clone();
    linker
        .func_wrap("env", "print_int", move |v: i32| e.print(&v.to_string()))
        .unwrap();
    let e = env.clone();
    linker
        .func_wrap("env", "print_float", move |v: f32| e.print(&v.to_string()))
        .unwrap();
    let e = env.clone();
    linker
        .func_wrap("env", "print_double", move |v: f64| e.print(&v.to_string()))
        .unwrap();
    let e = env.clone();
    linker
        .func_wrap("env", "print_char", move |v: i32| {
            if let Some(c) = char::from_u32(v as u32) {
                e.print(&c.to_string());
            }
        })
        .unwrap();
    let e = env.clone();
    linker
        .func_wrap(
            "env",
            "print_string",
            move |mut caller: Caller<'_, ()>, ptr: i32| {
                let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
                let s = read_string_from_memory(&memory, &caller, ptr);
                e.print(&s);
            },
        )
        .unwrap();

    link_math_functions(&mut linker).unwrap();
    link_file_functions(&mut linker).unwrap();
    link_http_functions(&mut linker).unwrap();
    link_regex_functions(&mut linker).unwrap();
    link_console_functions(&mut linker).unwrap();
    link_datetime_functions(&mut linker).unwrap();
    link_worker_functions(&mut linker).unwrap();

    linker
        .define_unknown_imports_as_traps(&module)
        .map_err(|e| format!("stub imports: {e}"))?;
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| format!("instantiate: {e}"))?;
    let main = instance
        .get_typed_func::<(), ()>(&mut store, "main")
        .map_err(|e| format!("no main: {e}"))?;
    main.call(&mut store, ())
        .map_err(|e| format!("execute: {e}"))?;

    let out = env.output.lock().unwrap().clone();
    Ok(out)
}

#[test]
fn mir_backend_e2e_coverage() {
    let cases_dir = Path::new("tests/cases");
    if !cases_dir.exists() {
        return;
    }
    let xfail: BTreeSet<&str> = XFAIL.iter().map(|(name, _)| *name).collect();

    let mut passed: Vec<String> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut unexpected_pass: Vec<String> = Vec::new();

    let mut entries: Vec<_> = fs::read_dir(cases_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("dream"))
        .collect();
    entries.sort();

    for path in entries {
        let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
        // Cases that are supposed to fail compilation are not backend-coverage cases.
        if path.with_extension("expected_error").exists() {
            continue;
        }
        let expected = match fs::read_to_string(path.with_extension("expected")) {
            Ok(s) => s,
            Err(_) => continue, // no golden output to compare against
        };

        let is_xfail = xfail.contains(stem.as_str());
        match compile_and_run_mir(&path) {
            Ok(actual) if actual.trim() == expected.trim() => {
                if is_xfail {
                    unexpected_pass.push(stem);
                } else {
                    passed.push(stem);
                }
            }
            Ok(actual) => {
                if !is_xfail {
                    failed.push((stem, format!("output mismatch: got {:?}", actual.trim())));
                }
            }
            Err(e) => {
                if !is_xfail {
                    failed.push((stem, e));
                }
            }
        }
    }

    eprintln!(
        "\nMIR backend e2e coverage: {} passing, {} xfail, {} unexpectedly failing",
        passed.len(),
        xfail.len(),
        failed.len()
    );
    eprintln!("passing: {passed:?}");

    if !unexpected_pass.is_empty() {
        eprintln!("\nThese XFAIL cases now PASS — remove them from XFAIL:\n  {unexpected_pass:?}");
    }
    if !failed.is_empty() {
        let detail: String = failed
            .iter()
            .map(|(n, e)| format!("  {n}: {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} case(s) not in XFAIL failed through the MIR backend:\n{detail}",
            failed.len()
        );
    }
    assert!(
        unexpected_pass.is_empty(),
        "XFAIL is stale (see message above)"
    );
}
