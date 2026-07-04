//! Shared test harness for the analyzer suite: the parse->analyze->emit pipeline helpers
//! (`compile_test_pipeline` and the `emit_*`/`run_*` wrappers built on it) plus the reusable
//! Dream source stubs (`SYSTEM_STUB`/`JS_STUB`/`ASYNC_STUB`). The `emission_tests` and
//! `analysis_tests` sibling modules `use super::harness::*` to reach these.

use super::super::*;
use crate::syntax::lexer::Lexer;
use crate::syntax::parser::Parser;

pub(super) fn analyze_code(code: &str) -> DiagnosticBag {
    let mut diagnostics = DiagnosticBag::new(None);
    let lexer = Lexer::new(code.to_string());
    let arena = bumpalo::Bump::new();
    let mut parser = Parser::new(lexer, &arena, &mut diagnostics);

    if let Ok(tree) = parser.parse() {
        let arena = bumpalo::Bump::new();
        let mut analyzer = Analyzer::new(&tree, &arena);
        let _ = analyzer.analyze(&mut diagnostics);
    }

    diagnostics
}

/// Parses and analyzes `code` (asserting parse + analysis succeed with no diagnostics) and hands the
/// resulting HIR and type interner to `emit`, returning whatever it produces. This is the shared
/// front half — parse -> analyze -> assert clean -> borrow the interner — that every emit helper
/// below otherwise duplicated; each now differs only in the `emit` closure (how it lowers/runs).
pub(super) fn compile_test_pipeline<R>(
    code: &str,
    emit: impl FnOnce(&crate::hir::Hir, &crate::types::TypeInterner) -> R,
) -> R {
    let mut diagnostics = DiagnosticBag::new(None);
    let lexer = Lexer::new(code.to_string());
    let parse_arena = bumpalo::Bump::new();
    let mut parser = Parser::new(lexer, &parse_arena, &mut diagnostics);
    let tree = parser.parse().expect("parse should succeed");
    let arena = bumpalo::Bump::new();
    let mut analyzer = Analyzer::new(&tree, &arena);
    let hir = analyzer
        .analyze(&mut diagnostics)
        .expect("analysis should succeed")
        .hir;
    assert!(!diagnostics.has_errors(), "unexpected analysis errors");
    let interner = &analyzer.type_ctx.interner;
    emit(&hir, interner)
}

/// Analyzes `code`, asserts it is error-free, and runs the *interleaved-emitted* HIR through the new
/// MIR backend (`lower -> passes -> emit`), returning the WAT and how many functions were emitted.
/// Exercises HIR emission end-to-end: source -> analyzer-emitted HIR -> WAT.
pub(super) fn emit_hir_to_wat(code: &str) -> (String, usize) {
    compile_test_pipeline(code, |hir, interner| {
        let count = hir.functions.len();
        let mut mir = crate::mir::lower::lower_program(hir, interner);

        let mut pm = crate::mir::passes::PassManager::new();
        pm.add(crate::mir::passes::CopyConstProp);
        pm.add(crate::mir::passes::ConstFold);
        pm.add(crate::mir::passes::SimplifyCfg);
        pm.add(crate::mir::passes::Dce);
        for f in &mut mir.functions {
            pm.run(f, interner);
        }
        (crate::mir::emit::emit_program(&mir, interner), count)
    })
}

/// Compiles `code` through the MIR backend, instantiates the module under wasmtime with the host
/// `print_*` imports wired to a capture buffer, runs the exported `entry`, and returns everything it
/// printed. This exercises the *runtime* — allocator, string ABI, and `*_to_string` — for real,
/// rather than only asserting the emitted text assembles.
#[cfg(feature = "native")]
pub(super) fn run_and_capture(code: &str, entry: &str) -> String {
    run_wat(&emit_hir_to_module(code), entry)
}

/// Like [`emit_hir_to_module`] but runs [`RcInsertion`] first, so `Retain`/`Release` statements are
/// present. Needed to exercise the deep-release runtime: `del()` fires when a reference's last owner
/// is released (here, when a reference local is overwritten). Only RC insertion is run — the
/// optimizing passes are skipped so they cannot elide the release we are testing.
#[cfg(feature = "native")]
pub(super) fn emit_hir_to_module_rc(code: &str) -> String {
    emit_hir_to_module_rc_only(code)
}

/// Compiles `code` with RC insertion enabled and runs it, capturing output (see [`run_and_capture`]).
#[cfg(feature = "native")]
pub(super) fn run_and_capture_rc(code: &str, entry: &str) -> String {
    run_wat(&emit_hir_to_module_rc(code), entry)
}

/// Instantiates a WAT module under wasmtime with the host `print_*` imports wired to a capture
/// buffer, runs the exported `entry`, and returns everything it printed. This exercises the *runtime*
/// — allocator, string ABI, `*_to_string`, and deep release — for real, not just that it assembles.
#[cfg(feature = "native")]
pub(super) fn run_wat(wat: &str, entry: &str) -> String {
    use std::sync::{Arc, Mutex};
    use wasmtime::*;

    let wasm = wat::parse_str(wat).expect("module should assemble");
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm).expect("module should compile");

    let out = Arc::new(Mutex::new(String::new()));
    let mut store = Store::new(&engine, out.clone());
    let mut linker = Linker::new(&engine);

    linker
        .func_wrap(
            "env",
            "print_int",
            |c: Caller<'_, Arc<Mutex<String>>>, v: i32| {
                c.data().lock().unwrap().push_str(&v.to_string());
            },
        )
        .unwrap();
    linker
        .func_wrap(
            "env",
            "print_char",
            |c: Caller<'_, Arc<Mutex<String>>>, v: i32| {
                if let Some(ch) = char::from_u32(v as u32) {
                    c.data().lock().unwrap().push(ch);
                }
            },
        )
        .unwrap();
    linker
        .func_wrap(
            "env",
            "print_float",
            |c: Caller<'_, Arc<Mutex<String>>>, v: f32| {
                c.data().lock().unwrap().push_str(&v.to_string());
            },
        )
        .unwrap();
    linker
        .func_wrap(
            "env",
            "print_double",
            |c: Caller<'_, Arc<Mutex<String>>>, v: f64| {
                c.data().lock().unwrap().push_str(&v.to_string());
            },
        )
        .unwrap();
    linker
        .func_wrap(
            "env",
            "print_string",
            |mut c: Caller<'_, Arc<Mutex<String>>>, ptr: i32| {
                let mem = c.get_export("memory").unwrap().into_memory().unwrap();
                let data = mem.data(&c);
                // Length-prefixed string: `[len: i32][utf8...][\0]` at the data pointer.
                let base = ptr as usize;
                let len = i32::from_le_bytes([
                    data[base],
                    data[base + 1],
                    data[base + 2],
                    data[base + 3],
                ]) as usize;
                let start = base + 4;
                let end = (start + len).min(data.len());
                let s = String::from_utf8_lossy(&data[start..end]).into_owned();
                c.data().lock().unwrap().push_str(&s);
            },
        )
        .unwrap();

    let instance = linker
        .instantiate(&mut store, &module)
        .expect("module should instantiate");
    let func = instance
        .get_typed_func::<(), ()>(&mut store, entry)
        .unwrap_or_else(|_| panic!("module should export `{}`", entry));
    func.call(&mut store, ())
        .expect("entry should run without trapping");
    let captured = out.lock().unwrap().clone();
    captured
}

/// Like [`emit_hir_to_wat`] but emits the full self-contained module (imports, memory, runtime,
/// exports) via `emit_module`, so import/scaffold concerns can be asserted and assembled.
pub(super) fn emit_hir_to_module(code: &str) -> String {
    compile_test_pipeline(code, |hir, interner| {
        let mir = crate::mir::lower::lower_program(hir, interner);
        crate::mir::emit::emit_module(&mir, interner, false)
    })
}

/// Like [`emit_hir_to_module`] but runs `RcInsertion` first (no other passes), matching the
/// production pipeline where reference-counting is always inserted before emission. Needed for tests
/// that assert on the deep-release runtime: those helper functions are only *reachable* — and so
/// retained by the module's dead-function elimination — once a `Release` call site references them.
pub(super) fn emit_hir_to_module_rc_only(code: &str) -> String {
    compile_test_pipeline(code, |hir, interner| {
        let mut mir = crate::mir::lower::lower_program(hir, interner);
        use crate::mir::passes::MirPass;
        for f in &mut mir.functions {
            crate::mir::passes::RcInsertion.run(f, interner);
        }
        crate::mir::emit::emit_module(&mir, interner, false)
    })
}

/// The `System` intrinsic surface (mirrors `stdlib/system/system.dream`), inlined so the print tests do not
/// depend on the full prelude being merged by the unit-test harness.
pub(super) const SYSTEM_STUB: &str = "
    class System {
        @intrinsic(\"print\")
        static extern fun print<T>(value: T): void;
        @intrinsic(\"println\")
        static extern fun println<T>(value: T): void;
    }
";

/// The dynamic-`js` bridge surface (mirrors `stdlib/core/js.dream`), inlined so the interop tests do
/// not depend on the full prelude being merged by the unit-test harness. `js` itself is a built-in
/// type; these `extend js` declarations provide the entry points and `@js` bridge externs the
/// analyzer desugars dynamic operations into.
pub(super) const JS_STUB: &str = "
    enum Option<T> {
        Some(value: T),
        None,
    }
    extend js {
        @js(\"Dream\", \"jsGlobal\")
        static extern fun global(name: string): js;
        @js(\"Dream\", \"jsGlobalThis\")
        static extern fun __global_this(): js;
        @js(\"Dream\", \"jsObject\")
        static extern fun object(): js;
        @js(\"Dream\", \"jsArray\")
        static extern fun array(): js;
        @js(\"Dream\", \"jsFunc\")
        static extern fun func(handler: fun(js): void): js;
        @js(\"Dream\", \"jsFunc0\")
        static extern fun func0(handler: fun(): void): js;
        @js(\"Dream\", \"jsInt\")
        static extern fun __box_int(value: int): js;
        @js(\"Dream\", \"jsLong\")
        static extern fun __box_long(value: long): js;
        @js(\"Dream\", \"jsDouble\")
        static extern fun __box_double(value: double): js;
        @js(\"Dream\", \"jsBool\")
        static extern fun __box_bool(value: bool): js;
        @js(\"Dream\", \"jsString\")
        static extern fun __box_string(value: string): js;
        @js(\"Dream\", \"jsGetV\")
        static extern fun __get(target: js, name: string): js;
        @js(\"Dream\", \"jsSetV\")
        static extern fun __set(target: js, name: string, value: js): void;
        @js(\"Dream\", \"jsCallV\")
        static extern fun __call(target: js, name: string, args: js[]): js;
        @js(\"Dream\", \"jsInvokeV\")
        static extern fun __invoke(target: js, args: js[]): js;
        @js(\"Dream\", \"jsIndexGetV\")
        static extern fun __index_get(target: js, key: js): js;
        @js(\"Dream\", \"jsIndexSetV\")
        static extern fun __index_set(target: js, key: js, value: js): void;
        @js(\"Dream\", \"jsAwait\")
        static extern async fun __await(target: js): js;
        @js(\"Dream\", \"jsAsInt\")
        static extern fun __as_int(target: js): int;
        @js(\"Dream\", \"jsAsDouble\")
        static extern fun __as_double(target: js): double;
        @js(\"Dream\", \"jsAsBool\")
        static extern fun __as_bool(target: js): bool;
        @js(\"Dream\", \"jsAsString\")
        static extern fun __as_string(target: js): string;
        public fun to_int(): int { return js.__as_int(this); }
        public fun to_str(): string { return js.__as_string(this); }
    }
";

/// `System` + `Time.sleep` for async tests (mirrors `stdlib/system/time.dream` + `system.dream`).
pub(super) const ASYNC_STUB: &str = "
    class System {
        @intrinsic(\"print\")
        static extern fun print<T>(value: T): void;
        @intrinsic(\"println\")
        static extern fun println<T>(value: T): void;
    }
    class Time {
        @intrinsic(\"sleep\")
        static extern async fun sleep(ms: int): void;
    }
";
