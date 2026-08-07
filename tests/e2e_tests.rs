use dream::driver::compiler::{Compiler, Target};
use dream::execution::host::{
    link_console_functions, link_crypto_functions, link_datetime_functions, link_file_functions,
    link_gpu_functions, link_http_functions, link_math_functions, link_process_functions,
    link_text_functions, link_worker_functions, read_string_from_memory, set_worker_module,
};
use pretty_assertions::assert_eq;
use rayon::prelude::*;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wasmtime::*;

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

/// Cases that read `Debug.live_objects()` / `total_allocations()`. Those probes only return real
/// counts when the allocator is instrumented (the default debug build), so they only produce
/// correct output in debug. The release suite runs these in debug (bypassing `--release`) rather
/// than release, so their full output stays asserted.
const DEBUG_ONLY_CASES: &[&str] = &[
    "struct_rc",
    "memory_advanced",
    "struct_container_rc",
    "value_union_option",
    "gc_complete",
];

fn run_test_case(dream_file: &Path, release: bool, wat_ext: &str) {
    let expected_file = dream_file.with_extension("expected");
    let expected_error_file = dream_file.with_extension("expected_error");
    let expected_trap_file = dream_file.with_extension("expected_trap");

    // Debug (default) enables allocator instrumentation and keeps every runtime helper; release
    // runs the same program through `strip_dead_functions` and the uninstrumented hot path, so this
    // second mode is what actually exercises structural WAT DCE.
    let compiler = Compiler::new(Target::Wasm).with_release(release);
    // Suite-scoped output path (passed in by the caller) so the debug and release suites never race
    // on the same file — crucial because both suites compile `DEBUG_ONLY_CASES` in debug mode, and
    // each suite additionally runs its corpus in parallel across cores.
    let wat_path = dream_file.with_extension(wat_ext);

    let dream_file_str = dream_file.to_str().unwrap().to_string();
    let wat_path_str = wat_path.to_str().unwrap().to_string();

    let compile_result = compiler.compile(&dream_file_str, &wat_path_str);

    if expected_error_file.exists() {
        let _expected_error = fs::read_to_string(&expected_error_file).unwrap();
        assert!(
            compile_result.is_err(),
            "Expected compilation to fail for {:?}",
            dream_file
        );
        // We could check the exact error message if we exposed it from Compiler,
        // but for now just ensuring it fails is good.
        return;
    }

    compile_result.unwrap_or_else(|_| panic!("Compilation failed for {:?}", dream_file));

    let expects_trap = expected_trap_file.exists();
    let expected_output = fs::read_to_string(if expects_trap {
        &expected_trap_file
    } else {
        &expected_file
    })
    .unwrap_or_else(|_| panic!("Missing .expected/.expected_trap file for {:?}", dream_file));

    let wat_content = fs::read_to_string(&wat_path).unwrap();

    // 2. Parse WAT to Wasm binary
    let wasm_bytes = wat::parse_str(&wat_content).expect("Failed to parse WAT");

    // Make the module bytes available to `WebWorker` spawns on this thread (a thread-local, so the
    // parallel debug/release suites never race on module identity).
    set_worker_module(&wasm_bytes);

    // 3. Setup Wasmtime
    // A recursive ARC release (e.g. dropping a long `Option<T>`-boxed linked list) chains one wasm
    // frame per node through both the struct's and its `Option` wrapper's release function, so the
    // default 512 KiB wasm stack undersizes for large-but-ordinary data structures; size up to match
    // the production runner (`execution::wasm_runner::execute_wasm`).
    let config = dream::execution::host::threaded_wasm_config();
    let engine = Engine::new(&config).expect("Failed to create engine");
    let module = Module::new(&engine, &wasm_bytes).expect("Failed to create module");
    let shared_mem = dream::execution::host::shared_memory_for(&engine, &module)
        .expect("module should import env.memory");
    dream::execution::host::set_worker_runtime(engine.clone(), shared_mem.clone());

    let mut store = Store::new(&engine, ());
    let mut linker = Linker::new(&engine);
    linker
        .define(&mut store, "env", "memory", shared_mem.clone())
        .expect("Failed to define shared memory");

    // 4. Setup Host Functions
    let env = TestEnv::new();

    // We need to extract memory later to read strings, so we'll pass it to host functions via a hack
    // Wasmtime allows accessing memory from Caller

    let env_clone = env.clone();
    linker
        .func_wrap("env", "print_int", move |v: i32| {
            env_clone.print(&v.to_string());
        })
        .unwrap();

    let env_clone = env.clone();
    linker
        .func_wrap("env", "print_float", move |v: f32| {
            env_clone.print(&v.to_string());
        })
        .unwrap();

    let env_clone = env.clone();
    linker
        .func_wrap("env", "print_double", move |v: f64| {
            env_clone.print(&v.to_string());
        })
        .unwrap();

    let env_clone = env.clone();
    linker
        .func_wrap("env", "print_char", move |v: i32| {
            if let Some(c) = char::from_u32(v as u32) {
                env_clone.print(&c.to_string());
            }
        })
        .unwrap();

    let env_clone = env.clone();
    linker
        .func_wrap(
            "env",
            "print_string",
            move |mut caller: Caller<'_, ()>, ptr: i32| {
                let memory = caller
                    .get_export("memory")
                    .unwrap()
                    .into_shared_memory()
                    .unwrap();
                let s = read_string_from_memory(&memory, ptr);
                env_clone.print(&s);
            },
        )
        .unwrap();

    linker
        .func_wrap("env", "concat_strings", |_: i32, _: i32| -> i32 {
            0 // Dummy implementation for now, full stdlib needs actual memory management
        })
        .unwrap();

    link_math_functions(&mut linker).unwrap();
    link_file_functions(&mut linker).unwrap();
    link_http_functions(&mut linker).unwrap();
    link_crypto_functions(&mut linker).unwrap();
    link_console_functions(&mut linker).unwrap();
    link_datetime_functions(&mut linker).unwrap();
    link_process_functions(&mut linker).unwrap();
    link_text_functions(&mut linker).unwrap();
    link_worker_functions(&mut linker).unwrap();
    link_gpu_functions(&mut linker).unwrap();
    linker
        .func_wrap("env", "strlen", |_: i32| -> i32 { 0 })
        .unwrap();
    linker
        .func_wrap("env", "malloc", |_: i32| -> i32 { 0 })
        .unwrap();
    linker.func_wrap("env", "free", |_: i32| {}).unwrap();

    linker
        .func_wrap("env", "debug_get_free_list_head", move || -> i32 {
            // We can't easily get the freelist head from here without exporting it,
            // but we can just return 0 to make the linker happy if it's not actually used
            // or if we just want to stub it.
            // Actually, let's just return 0 for now. The test checks if it changes.
            0
        })
        .unwrap();

    // 5. Instantiate and Run
    // JS-interop externs (the `Dream` host module behind the dynamic `js` type/regex/fetch, plus any user
    // `@js(...)` imports) are merged in via the prelude but have no native host here. Stub every
    // unresolved import as a trap so pure-Dream cases still instantiate; they never call them.
    linker
        .define_unknown_imports_as_traps(&module)
        .expect("Failed to stub unknown imports");
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("Failed to instantiate");
    let main_func = instance
        .get_typed_func::<(), ()>(&mut store, "main")
        .expect("Failed to get main function");

    let result = main_func.call(&mut store, ());
    if expects_trap {
        assert!(
            result.is_err(),
            "Expected a runtime panic (trap) for {:?}, but execution completed normally",
            dream_file
        );
    } else {
        result.expect("Execution failed");
    }

    // 6. Assert Output (a `panic(msg)` prints its message, then the newline `$dream_panic` appends,
    // before the `unreachable` trap unwinds execution — so the printed text is still asserted).
    let actual_output = env.output.lock().unwrap().clone();
    if expects_trap {
        // Automatic-check panic messages are *located* with the source file's canonicalized
        // absolute path (see `panic_msgs::located`), which is not reproducible across checkouts —
        // so `.expected_trap` only pins the message's fixed prefix (everything up to and including
        // the base message), not the trailing `(at <path>, in <function>)` suffix.
        assert!(
            actual_output.trim().starts_with(expected_output.trim()),
            "Output mismatch for {:?}\n  expected prefix: {:?}\n  actual: {:?}",
            dream_file,
            expected_output.trim(),
            actual_output.trim()
        );
    } else {
        assert_eq!(
            actual_output.trim(),
            expected_output.trim(),
            "Output mismatch for {:?}",
            dream_file
        );
    }

    // Cleanup generated WAT
    let _ = fs::remove_file(wat_path);
}

/// Collect every `tests/cases/*.dream` fixture path.
fn collect_case_paths() -> Vec<PathBuf> {
    let cases_dir = Path::new("tests/cases");
    if !cases_dir.exists() {
        return Vec::new();
    }
    fs::read_dir(cases_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("dream"))
        .collect()
}

/// Run the whole corpus in parallel across all CPU cores. `release_for` decides, per fixture stem,
/// whether the case runs through the release or debug backend. `wat_ext` scopes this suite's
/// generated output files so concurrent suites never collide. Each fixture's failure is captured
/// (rather than aborting the run at the first panic) so one invocation reports every broken case.
fn run_corpus(wat_ext: &str, release_for: impl Fn(&str) -> bool + Sync) {
    let paths = collect_case_paths();
    if paths.is_empty() {
        println!("No .dream files found in tests/cases/");
        return;
    }

    let failures: Vec<String> = paths
        .par_iter()
        .filter_map(|path| {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let release = release_for(stem);
            // `run_test_case` signals failure via panic/assert; catch it so rayon aggregates rather
            // than tearing down the whole run on the first failure.
            match catch_unwind(AssertUnwindSafe(|| run_test_case(path, release, wat_ext))) {
                Ok(()) => None,
                Err(payload) => {
                    let msg = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "unknown panic".to_string());
                    Some(format!("{:?}: {}", path, msg))
                }
            }
        })
        .collect();

    assert!(
        failures.is_empty(),
        "{} e2e case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn run_all_e2e_cases() {
    // Default debug backend (`with_release(false)`).
    run_corpus("wat", |_| false);
}

/// The whole suite run through the *release* backend (`with_release(true)`), the only path that
/// enables structural WAT dead-function elimination and the uninstrumented allocator. This guards
/// against a case that passes in debug but breaks in release because DCE trimmed a live function or
/// the hot path diverged. EVERY case runs here with full output asserted: instrumentation-probe
/// cases (`DEBUG_ONLY_CASES`) run in debug (their counts are debug-specific), all others in release.
#[test]
fn run_all_e2e_cases_release() {
    // The instrumentation-probe cases only produce correct output with the debug allocator, so
    // bypass release for them and run them in debug with the full output assertion — they are
    // important and must stay fully checked, not relaxed to a smoke test.
    run_corpus("release.wat", |stem| !DEBUG_ONLY_CASES.contains(&stem));
}

/// Codegen must be reproducible: compiling the same program twice (each compile uses fresh,
/// independently-seeded `HashMap`s within this process) must yield byte-identical `.wat` and
/// (with `--runtime`) `*.web.runtime.js`. This guards the `IndexMap` conversion of the emission-driving
/// tables against regressions that would reintroduce `HashMap`-iteration nondeterminism. Uses
/// release mode so the check covers the production emit path (`strip_dead_functions`).
#[test]
fn codegen_is_deterministic() {
    let cases_dir = Path::new("tests/cases");
    if !cases_dir.exists() {
        return;
    }
    // Exercise structs, enums, discriminated unions, generics, strings, and the object protocol.
    let fixtures = [
        "structs",
        "enum_basic",
        "union_to_string",
        "generic_structs",
        "json_derive",
    ];
    for name in fixtures {
        let src = cases_dir.join(format!("{}.dream", name));
        if !src.exists() {
            continue;
        }
        let src_str = src.to_str().unwrap().to_string();
        let mut prev_wat: Option<String> = None;
        let mut prev_rt: Option<String> = None;
        for run in 0..4 {
            let out = std::env::temp_dir().join(format!("dream_det_{}_{}.wat", name, run));
            let out_str = out.to_str().unwrap().to_string();
            Compiler::new(Target::Wasm)
                .with_release(true)
                .with_runtimes(vec![dream::driver::js_runtime::JsRuntimeTarget::Web])
                .compile(&src_str, &out_str)
                .unwrap_or_else(|_| panic!("Compilation failed for {}", name));
            let wat = fs::read_to_string(&out).unwrap();
            let rt_path = out.with_extension("web.runtime.js");
            let rt = fs::read_to_string(&rt_path).unwrap_or_else(|e| {
                panic!("missing selective runtime for {}: {}", name, e)
            });
            let _ = fs::remove_file(&out);
            let _ = fs::remove_file(&rt_path);
            let _ = fs::remove_file(out.with_extension("wasm"));
            let _ = fs::remove_file(out.with_extension("abi.json"));
            if let Some(ref first) = prev_wat {
                assert_eq!(
                    first, &wat,
                    "Nondeterministic codegen for {} (run {})",
                    name, run
                );
            } else {
                prev_wat = Some(wat);
            }
            if let Some(ref first) = prev_rt {
                assert_eq!(
                    first, &rt,
                    "Nondeterministic selective runtime for {} (run {})",
                    name, run
                );
            } else {
                prev_rt = Some(rt);
            }
        }
    }
}

/// `runtime/dream.js` must match a fresh bundle of `runtime/src/` (edit sources, then run
/// `node scripts/bundle-runtime.mjs`).
#[test]
fn dream_js_bundle_is_fresh() {
    let status = std::process::Command::new("node")
        .args(["scripts/bundle-runtime.mjs", "--check"])
        .status()
        .expect("failed to spawn node for bundle-runtime check");
    assert!(
        status.success(),
        "runtime/dream.js is stale; run: node scripts/bundle-runtime.mjs"
    );
}

/// A compute-free arithmetic program must not pull GPU/FS/crypto host chunks into its selective
/// runtime (js bridges may still appear when layouts exist for marshaler keepalive).
#[test]
fn selective_runtime_omits_unused_host_chunks() {
    let src = Path::new("tests/cases/arithmetic.dream");
    if !src.exists() {
        return;
    }
    let out = std::env::temp_dir().join("dream_sel_runtime_check.wat");
    let out_str = out.to_str().unwrap().to_string();
    let src_str = src.to_str().unwrap().to_string();
    Compiler::new(Target::Wasm)
        .with_runtimes(vec![dream::driver::js_runtime::JsRuntimeTarget::Web])
        .compile(&src_str, &out_str)
        .expect("arithmetic compile");
    let rt = fs::read_to_string(out.with_extension("web.runtime.js")).expect("web.runtime.js");
    let _ = fs::remove_file(&out);
    let _ = fs::remove_file(out.with_extension("web.runtime.js"));
    let _ = fs::remove_file(out.with_extension("wasm"));
    let _ = fs::remove_file(out.with_extension("abi.json"));
    assert!(!rt.contains("makeGpuHost"), "gpu chunk should be absent");
    assert!(!rt.contains("makeFsHost"), "fs chunk should be absent");
    assert!(!rt.contains("makeCryptoHost"), "crypto chunk should be absent");
    assert!(rt.contains("function load("));
}
