use dream::driver::compiler::{Compiler, Target};
use dream::execution::host::{
    link_console_functions, link_datetime_functions, link_file_functions, link_http_functions,
    link_math_functions, link_regex_functions, link_worker_functions, read_string_from_memory,
    set_worker_module,
};
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;
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
/// counts when the allocator is instrumented (`--debug`), so they only produce correct output in
/// debug. The release suite runs these in debug (bypassing release) rather than release, so their
/// full output stays asserted.
const DEBUG_ONLY_CASES: &[&str] = &[
    "struct_rc",
    "memory_advanced",
    "struct_container_rc",
    "value_union_option",
    "gc_complete",
];

fn run_test_case(dream_file: &Path, debug: bool) {
    let expected_file = dream_file.with_extension("expected");
    let expected_error_file = dream_file.with_extension("expected_error");

    // Debug enables allocator instrumentation (so GC/leak probes report real counts) and keeps every
    // runtime helper; release runs the same program through `strip_dead_functions` and the
    // uninstrumented hot path, so this second mode is what actually exercises structural WAT DCE.
    let compiler = Compiler::new(Target::Wasm).with_debug(debug);
    // Mode-specific output path so the debug and release passes never race on the same file when
    // cargo runs the two suite tests in parallel.
    let wat_ext = if debug { "wat" } else { "release.wat" };
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

    let expected_output = fs::read_to_string(&expected_file)
        .unwrap_or_else(|_| panic!("Missing .expected file for {:?}", dream_file));

    let wat_content = fs::read_to_string(&wat_path).unwrap();

    // 2. Parse WAT to Wasm binary
    let wasm_bytes = wat::parse_str(&wat_content).expect("Failed to parse WAT");

    // Make the module bytes available to `WebWorker` spawns on this thread (a thread-local, so the
    // parallel debug/release suites never race on module identity).
    set_worker_module(&wasm_bytes);

    // 3. Setup Wasmtime
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm_bytes).expect("Failed to create module");

    let mut store = Store::new(&engine, ());
    let mut linker = Linker::new(&engine);

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
                let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
                let s = read_string_from_memory(&memory, &caller, ptr);
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
    link_regex_functions(&mut linker).unwrap();
    link_console_functions(&mut linker).unwrap();
    link_datetime_functions(&mut linker).unwrap();
    link_worker_functions(&mut linker).unwrap();
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

    main_func.call(&mut store, ()).expect("Execution failed");

    // 6. Assert Output
    let actual_output = env.output.lock().unwrap().clone();
    assert_eq!(
        actual_output.trim(),
        expected_output.trim(),
        "Output mismatch for {:?}",
        dream_file
    );

    // Cleanup generated WAT
    let _ = fs::remove_file(wat_path);
}

#[test]
fn run_all_e2e_cases() {
    let cases_dir = Path::new("tests/cases");
    if !cases_dir.exists() {
        return;
    }

    let mut ran_any = false;
    for entry in fs::read_dir(cases_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("dream") {
            println!("Running E2E test: {:?}", path);
            run_test_case(&path, true);
            ran_any = true;
        }
    }

    if !ran_any {
        println!("No .dream files found in tests/cases/");
    }
}

/// The whole suite run through the *release* backend (`with_debug(false)`), the only path that
/// enables structural WAT dead-function elimination and the uninstrumented allocator. This guards
/// against a case that passes in debug but breaks in release because DCE trimmed a live function or
/// the hot path diverged. EVERY case runs here with full output asserted: instrumentation-probe
/// cases (`DEBUG_ONLY_CASES`) run in debug (their counts are debug-specific), all others in release.
#[test]
fn run_all_e2e_cases_release() {
    let cases_dir = Path::new("tests/cases");
    if !cases_dir.exists() {
        return;
    }

    for entry in fs::read_dir(cases_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("dream") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        // The instrumentation-probe cases only produce correct output with the debug allocator, so
        // bypass release for them and run them in debug with the full output assertion — they are
        // important and must stay fully checked, not relaxed to a smoke test.
        let debug = DEBUG_ONLY_CASES.contains(&stem);
        println!("Running release E2E test: {:?} (debug={})", path, debug);
        run_test_case(&path, debug);
    }
}

/// Codegen must be reproducible: compiling the same program twice (each compile uses fresh,
/// independently-seeded `HashMap`s within this process) must yield byte-identical `.wat`. This
/// guards the `IndexMap` conversion of the emission-driving tables against regressions that would
/// reintroduce `HashMap`-iteration nondeterminism.
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
        let mut prev: Option<String> = None;
        for run in 0..4 {
            let out = std::env::temp_dir().join(format!("dream_det_{}_{}.wat", name, run));
            let out_str = out.to_str().unwrap().to_string();
            Compiler::new(Target::Wasm)
                .compile(&src_str, &out_str)
                .unwrap_or_else(|_| panic!("Compilation failed for {}", name));
            let wat = fs::read_to_string(&out).unwrap();
            let _ = fs::remove_file(&out);
            if let Some(ref first) = prev {
                assert_eq!(
                    first, &wat,
                    "Nondeterministic codegen for {} (run {})",
                    name, run
                );
            } else {
                prev = Some(wat);
            }
        }
    }
}
