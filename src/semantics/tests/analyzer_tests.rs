use super::*;
use crate::syntax::lexer::Lexer;
use crate::syntax::parser::Parser;
use pretty_assertions::assert_eq;

fn analyze_code(code: &str) -> DiagnosticBag {
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

/// Analyzes `code`, asserts it is error-free, and runs the *interleaved-emitted* HIR through the new
/// MIR backend (`lower -> passes -> emit`), returning the WAT and how many functions were emitted.
/// Exercises HIR emission end-to-end: source -> analyzer-emitted HIR -> WAT.
fn emit_hir_to_wat(code: &str) -> (String, usize) {
    let mut diagnostics = DiagnosticBag::new(None);
    let lexer = Lexer::new(code.to_string());
    let parse_arena = bumpalo::Bump::new();
    let mut parser = Parser::new(lexer, &parse_arena, &mut diagnostics);
    let tree = parser.parse().expect("parse should succeed");

    let arena = bumpalo::Bump::new();
    let mut analyzer = Analyzer::new(&tree, &arena);
    let hir = {
        let info = analyzer.analyze(&mut diagnostics).expect("analysis should succeed");
        info.hir
    };
    assert!(!diagnostics.has_errors(), "unexpected analysis errors");

    let interner = &analyzer.type_ctx.interner;
    let count = hir.functions.len();
    let mut mir = crate::mir::lower::lower_program(&hir, interner);

    let mut pm = crate::mir::passes::PassManager::new();
    pm.add(crate::mir::passes::CopyConstProp);
    pm.add(crate::mir::passes::ConstFold);
    pm.add(crate::mir::passes::SimplifyCfg);
    pm.add(crate::mir::passes::Dce);
    for f in &mut mir.functions {
        pm.run(f, interner);
    }
    (crate::mir::emit::emit_program(&mir, interner), count)
}

/// Compiles `code` through the MIR backend, instantiates the module under wasmtime with the host
/// `print_*` imports wired to a capture buffer, runs the exported `entry`, and returns everything it
/// printed. This exercises the *runtime* — allocator, string ABI, and `*_to_string` — for real,
/// rather than only asserting the emitted text assembles.
#[cfg(feature = "native")]
fn run_and_capture(code: &str, entry: &str) -> String {
    run_wat(&emit_hir_to_module(code), entry)
}

/// Like [`emit_hir_to_module`] but runs [`RcInsertion`] first, so `Retain`/`Release` statements are
/// present. Needed to exercise the deep-release runtime: `del()` fires when a reference's last owner
/// is released (here, when a reference local is overwritten). Only RC insertion is run — the
/// optimizing passes are skipped so they cannot elide the release we are testing.
#[cfg(feature = "native")]
fn emit_hir_to_module_rc(code: &str) -> String {
    let mut diagnostics = DiagnosticBag::new(None);
    let lexer = Lexer::new(code.to_string());
    let parse_arena = bumpalo::Bump::new();
    let mut parser = Parser::new(lexer, &parse_arena, &mut diagnostics);
    let tree = parser.parse().expect("parse should succeed");
    let arena = bumpalo::Bump::new();
    let mut analyzer = Analyzer::new(&tree, &arena);
    let hir = analyzer.analyze(&mut diagnostics).expect("analysis should succeed").hir;
    assert!(!diagnostics.has_errors(), "unexpected analysis errors");
    let interner = &analyzer.type_ctx.interner;
    let mut mir = crate::mir::lower::lower_program(&hir, interner);
    use crate::mir::passes::MirPass;
    for f in &mut mir.functions {
        crate::mir::passes::RcInsertion.run(f, interner);
    }
    crate::mir::emit::emit_module(&mir, interner, false)
}

/// Compiles `code` with RC insertion enabled and runs it, capturing output (see [`run_and_capture`]).
#[cfg(feature = "native")]
fn run_and_capture_rc(code: &str, entry: &str) -> String {
    run_wat(&emit_hir_to_module_rc(code), entry)
}

/// Instantiates a WAT module under wasmtime with the host `print_*` imports wired to a capture
/// buffer, runs the exported `entry`, and returns everything it printed. This exercises the *runtime*
/// — allocator, string ABI, `*_to_string`, and deep release — for real, not just that it assembles.
#[cfg(feature = "native")]
fn run_wat(wat: &str, entry: &str) -> String {
    use std::sync::{Arc, Mutex};
    use wasmtime::*;

    let wasm = wat::parse_str(wat).expect("module should assemble");
    let engine = Engine::default();
    let module = Module::new(&engine, &wasm).expect("module should compile");

    let out = Arc::new(Mutex::new(String::new()));
    let mut store = Store::new(&engine, out.clone());
    let mut linker = Linker::new(&engine);

    linker
        .func_wrap("env", "print_int", |c: Caller<'_, Arc<Mutex<String>>>, v: i32| {
            c.data().lock().unwrap().push_str(&v.to_string());
        })
        .unwrap();
    linker
        .func_wrap("env", "print_char", |c: Caller<'_, Arc<Mutex<String>>>, v: i32| {
            if let Some(ch) = char::from_u32(v as u32) {
                c.data().lock().unwrap().push(ch);
            }
        })
        .unwrap();
    linker
        .func_wrap("env", "print_float", |c: Caller<'_, Arc<Mutex<String>>>, v: f32| {
            c.data().lock().unwrap().push_str(&v.to_string());
        })
        .unwrap();
    linker
        .func_wrap("env", "print_double", |c: Caller<'_, Arc<Mutex<String>>>, v: f64| {
            c.data().lock().unwrap().push_str(&v.to_string());
        })
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

    let instance = linker.instantiate(&mut store, &module).expect("module should instantiate");
    let func = instance
        .get_typed_func::<(), ()>(&mut store, entry)
        .unwrap_or_else(|_| panic!("module should export `{}`", entry));
    func.call(&mut store, ()).expect("entry should run without trapping");
    let captured = out.lock().unwrap().clone();
    captured
}

/// Like [`emit_hir_to_wat`] but emits the full self-contained module (imports, memory, runtime,
/// exports) via `emit_module`, so import/scaffold concerns can be asserted and assembled.
fn emit_hir_to_module(code: &str) -> String {
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
    let mir = crate::mir::lower::lower_program(&hir, interner);
    crate::mir::emit::emit_module(&mir, interner, false)
}

/// Like [`emit_hir_to_module`] but runs `RcInsertion` first (no other passes), matching the
/// production pipeline where reference-counting is always inserted before emission. Needed for tests
/// that assert on the deep-release runtime: those helper functions are only *reachable* — and so
/// retained by the module's dead-function elimination — once a `Release` call site references them.
fn emit_hir_to_module_rc_only(code: &str) -> String {
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
    let mut mir = crate::mir::lower::lower_program(&hir, interner);
    use crate::mir::passes::MirPass;
    for f in &mut mir.functions {
        crate::mir::passes::RcInsertion.run(f, interner);
    }
    crate::mir::emit::emit_module(&mir, interner, false)
}

#[test]
fn test_hir_emission_arithmetic_function() {
    // A plain free function over arithmetic on parameters is fully representable in HIR, so the
    // analyzer emits it and it survives the whole new backend pipeline.
    let (wat, count) = emit_hir_to_wat("fun add(a: int, b: int): int { return a + b; }");
    assert_eq!(count, 1, "the single free function should be emitted as HIR");
    assert!(wat.contains("(func $add"), "missing emitted function:\n{}", wat);
    assert!(wat.contains("i32.add"), "missing arithmetic:\n{}", wat);
}

#[test]
fn test_hir_emission_locals_and_assignment() {
    // `let` + assignment + return over locals: each statement is supported, so the function emits.
    let code = "fun calc(n: int): int { let x: int = n; let y: int = x + 1; y = y + n; return y; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1);
    assert!(wat.contains("(func $calc"), "missing emitted function:\n{}", wat);
}

#[test]
fn test_hir_emission_skips_unsupported_functions() {
    // An uninstantiated generic template (`gen<T>`) has no concrete body to lower until it is
    // monomorphized at a call site, so the interleaved HIR emission skips it, leaving the legacy path
    // to handle its instantiations. The concrete sibling still emits.
    let code = "
        fun simple(a: int): int { return a; }
        fun gen<T>(x: T): T { return x; }
    ";
    let (_, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "only the fully-supported function should be emitted");
}

#[test]
fn test_hir_emission_while_loop() {
    // `while` over locals is now fully representable; the whole function survives the pipeline and
    // its CFG is emitted via the block-dispatch loop.
    let code = "fun count(n: int): int { let s: int = 0; while (s < n) { s = s + 1; } return s; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the while function should be emitted as HIR");
    assert!(wat.contains("(func $count"), "missing emitted function:\n{}", wat);
    assert!(wat.contains("i32.lt_s"), "missing loop comparison:\n{}", wat);
    assert!(wat.contains("br_table"), "missing CFG dispatch:\n{}", wat);
}

#[test]
fn test_hir_emission_if_else_chain() {
    // `if` / `else if` / `else` folds into nested HIR `If`s and lowers to a branching CFG.
    let code = "
        fun classify(n: int): int {
            if (n < 0) { return 0; } else if (n == 0) { return 1; } else { return 2; }
        }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the if/else-if/else function should be emitted as HIR");
    assert!(wat.contains("(func $classify"), "missing emitted function:\n{}", wat);
}

#[test]
fn test_hir_emission_for_loop() {
    // A C-style `for (init; cond; step)` desugars to HIR `For` and lowers cleanly.
    let code = "
        fun sum(n: int): int {
            let acc: int = 0;
            for (let i: int = 0; i < n; i = i + 1) { acc = acc + i; }
            return acc;
        }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the for-loop function should be emitted as HIR");
    assert!(wat.contains("(func $sum"), "missing emitted function:\n{}", wat);
    assert!(wat.contains("i32.add"), "missing arithmetic:\n{}", wat);
}

#[test]
fn test_hir_emission_foreach_loop() {
    // For-each over an array parameter lowers to the indexed-iteration MIR form.
    let code = "
        fun total(xs: int[]): int {
            let acc: int = 0;
            for (let x in xs) { acc = acc + x; }
            return acc;
        }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the foreach function should be emitted as HIR");
    assert!(wat.contains("(func $total"), "missing emitted function:\n{}", wat);
}

#[test]
fn test_hir_emission_logical_and_ternary() {
    // `&&`/`||` lower to short-circuit control flow; the ternary lowers to a branch + join temp.
    let code = "
        fun pick(a: bool, b: bool, x: int, y: int): int {
            return (a && b) ? x : y;
        }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the logical/ternary function should be emitted as HIR");
    assert!(wat.contains("(func $pick"), "missing emitted function:\n{}", wat);
}

#[test]
fn test_hir_emission_coalesce() {
    // `lhs ?? rhs` lowers to a null-test branch joining into one temp.
    let code = "fun or_default(x: string?): string { return x ?? \"d\"; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the coalesce function should be emitted as HIR");
    assert!(wat.contains("(func $or_default"), "missing emitted function:\n{}", wat);
}

#[test]
fn test_hir_emission_cast() {
    // A numeric widening cast lowers to a concrete conversion instruction.
    let code = "fun widen(x: int): double { return (double)x; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the cast function should be emitted as HIR");
    assert!(wat.contains("f64.convert_i32_s"), "missing widening cast:\n{}", wat);
}

#[test]
fn test_hir_emission_index_and_array_literal() {
    // Array literals allocate via `$malloc` and store the length + elements; indexing reads through
    // the element address.
    let code = "
        fun first(xs: int[]): int { return xs[0]; }
        fun make(): int[] { return [1, 2, 3]; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both the index and array-literal functions should be emitted");
    assert!(wat.contains("(func $first"), "missing index function:\n{}", wat);
    assert!(wat.contains("(func $make"), "missing array-literal function:\n{}", wat);
    assert!(wat.contains("(call $malloc)"), "array literal should allocate:\n{}", wat);
}

#[test]
fn test_hir_emission_direct_call() {
    // A direct free-function call resolves to the callee's `DefId` and emits a `call`.
    let code = "
        fun addup(a: int, b: int): int { return a + b; }
        fun driver(): int { return addup(1, 2); }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both the callee and the caller should be emitted");
    assert!(wat.contains("(func $driver"), "missing caller:\n{}", wat);
    assert!(wat.contains("(call $addup"), "call should resolve to the callee symbol:\n{}", wat);
}

#[test]
fn test_hir_emission_extend_nongeneric_class() {
    // An `extend` method is lowered exactly like a struct method (`{Type}_{method}` + `this`), so its
    // body emits and an instance call resolves to it.
    let code = "
        class Point { public x: int; }
        extend Point { public fun getx(): int { return this.x; } }
        fun use_ext(p: Point): int { return p.getx(); }
    ";
    let (wat, _count) = emit_hir_to_wat(code);
    assert!(wat.contains("(func $Point_getx"), "extend method body should emit:\n{}", wat);
    assert!(wat.contains("(call $Point_getx"), "call should resolve to the extend method:\n{}", wat);
}

#[test]
fn test_hir_emission_extend_generic_class() {
    // A generic `extend Box<T>` monomorphizes alongside the struct instance: the method is registered
    // under the mangled name (`Box_int_peek`), so its body and call resolve there with no suffix.
    let code = "
        class Box<T> { public v: T; }
        extend Box<T> { public fun peek(): T { return this.v; } }
        fun use_ext(b: Box<int>): int { return b.peek(); }
    ";
    let (wat, _count) = emit_hir_to_wat(code);
    assert!(wat.contains("(func $Box_int_peek"), "generic extend method should emit:\n{}", wat);
    assert!(wat.contains("(call $Box_int_peek"), "call should resolve to the instance:\n{}", wat);
    assert!(!wat.contains("$Box_int_peek__"), "no instance suffix on a struct-generic extend:\n{}", wat);
}

#[test]
fn test_hir_emission_destructor_body() {
    // A `del()` destructor is lowered like any method, so its body emits under `{Type}_del`. (The
    // release-time *invocation* is part of the RC runtime and handled at the driver switch.)
    let code = "
        class Res { public h: int; del() { this.h = 0; } }
        fun mk(): Res { return Res(); }
    ";
    let (wat, _count) = emit_hir_to_wat(code);
    assert!(wat.contains("(func $Res_del"), "destructor body should emit:\n{}", wat);
}

#[test]
fn test_release_runtime_deep_release_del_and_dispatch() {
    // The deep-release runtime: each nominal type gets a `$release_<Type>` that (when the count hits
    // zero) runs its `del()` destructor, releases reference fields, and frees. `$release_object`
    // tag-dispatches to those per-type releases. Non-reference fields (`v: int`) are not released.
    let code = format!(
        "{SYSTEM_STUB}
        class Node {{ public next: Node?; public v: int;
            del() {{ System.print(0); }}
            constructor(v: int) {{ this.v = v; }}
        }}
        fun main(): void {{ let n: Node = Node(1); let o: object = n; }}"
    );
    // RC insertion is required so `main`'s scope-exit `Release`s reference the deep-release runtime;
    // dead-function elimination otherwise (correctly) drops those uncalled helpers. Binding to an
    // `object` local forces a statically-untyped release, exercising the tag-dispatch router.
    let wat = emit_hir_to_module_rc_only(&code);
    assert!(wat.contains("(func $release_Node"), "per-type release missing:\n{}", wat);
    assert!(wat.contains("(call $Node_del)"), "destructor not invoked from release:\n{}", wat);
    // The reference field `next` is deep-released; the scalar `v` is not.
    assert!(wat.contains("(call $release_Node)"), "reference field not released:\n{}", wat);
    assert!(wat.contains("(func $release_object"), "tag-dispatch router missing:\n{}", wat);
    assert!(wat.contains("(call $free)"), "release must free the block:\n{}", wat);
}

#[test]
fn test_hir_emission_user_constructor() {
    // A struct with a user-defined `constructor(){}`: `Point(1, 2)` allocates, zeroes, and calls the
    // constructor (rather than initializing fields positionally); the constructor body is emitted too.
    let code = "
        class Point {
            public x: int;
            public y: int;
            constructor(a: int, b: int) { this.x = a; this.y = b; }
        }
        fun make(): Point { return Point(1, 2); }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both the constructor body and make should be emitted:\n{}", wat);
    assert!(wat.contains("(func $Point_constructor"), "constructor body should emit:\n{}", wat);
    assert!(wat.contains("(call $malloc)"), "construction should allocate:\n{}", wat);
    assert!(
        wat.contains("(call $Point_constructor"),
        "construction should invoke the user constructor:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_generic_struct_construction_and_field() {
    // Constructing and reading a generic struct instance (`Box<int>`) resolves to the monomorphized
    // layout: `Box<int>(7)` allocates + stores the field, and `b.v` loads it. The per-instance
    // layout is keyed by the interned type, so field widths are correct.
    let code = "
        class Box<T> { public v: T; constructor(v: T) { this.v = v; } }
        fun make(): Box<int> { return Box<int>(7); }
        fun read(b: Box<int>): int { return b.v; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 3, "make, read, and the constructor body should be emitted:\n{}", wat);
    assert!(wat.contains("(call $malloc)"), "generic construction should allocate:\n{}", wat);
    assert!(wat.contains("(i32.store)"), "the field should be initialized:\n{}", wat);
    assert!(wat.contains("(i32.load)"), "the field read should lower to a load:\n{}", wat);
}

#[test]
fn test_hir_emission_generic_struct_method_instance() {
    // A method on a generic struct is a non-generic method whose specialization is baked into its
    // mangled def name (`Box_int_get`), so its body and call site resolve to that name with no
    // instance suffix — no `def{N}` fallback.
    let code = "
        class Box<T> { public v: T; public fun get(): T { return this.v; } }
        fun use_box(b: Box<int>): int { return b.get(); }
    ";
    let (wat, _count) = emit_hir_to_wat(code);
    assert!(
        wat.contains("(func $Box_int_get"),
        "generic-struct method body should emit under its mangled name:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $Box_int_get"),
        "instance call should dispatch to the mangled method:\n{}",
        wat
    );
    assert!(
        !wat.contains("$Box_int_get__"),
        "a struct-generic method should NOT carry an instance suffix:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_global_initializer_runs_in_start() {
    // A top-level variable's initializer is captured as the global's `init`; the module synthesizes
    // a `$__dream_init` that stores it and wires it to `(start ...)`, and the module assembles.
    let code = "
        let counter: int = 40;
        fun get(): int { return counter; }
    ";
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
    let mir = crate::mir::lower::lower_program(&hir, interner);
    let wat = crate::mir::emit::emit_module(&mir, interner, false);
    assert!(wat.contains("(func $__dream_init"), "missing init function:\n{}", wat);
    assert!(wat.contains("(start $__dream_init)"), "init must run at start:\n{}", wat);
    assert!(wat.contains("(global.set $g0)"), "init should store the global:\n{}", wat);
    wat::parse_str(&wat).expect("module with a start-based initializer should assemble");
}

#[test]
fn test_hir_emission_host_print_imports_present() {
    // Every module declares the fixed `print_*` host builtins (what `print`/`println` lower to), so
    // a program that uses none of them still emits the import prelude and assembles.
    let wat = emit_hir_to_module("fun get(): int { return 1; }");
    for name in ["print_string", "print_int", "print_float", "print_double", "print_char"] {
        assert!(
            wat.contains(&format!("(import \"env\" \"{name}\" (func ${name}")),
            "missing host import {name}:\n{}",
            wat
        );
    }
    wat::parse_str(&wat).expect("module with the host import prelude should assemble");
}

#[test]
fn test_hir_emission_extern_import_and_call() {
    // An `extern fun` becomes a WASM import (module/field from `@js`), and a call to it resolves to
    // the imported `$name` so the module links and assembles.
    let code = "
        @js(\"host\", \"log_it\")
        extern fun log(x: int): void;
        fun run(): void { log(7); }
    ";
    let wat = emit_hir_to_module(code);
    assert!(
        wat.contains("(import \"host\" \"log_it\" (func $log (param i32)))"),
        "extern should import from its @js target:\n{}",
        wat
    );
    assert!(wat.contains("(call $log)"), "call should resolve to the import:\n{}", wat);
    wat::parse_str(&wat).expect("module importing and calling an extern should assemble");
}

#[test]
fn test_hir_emission_extern_import_with_result() {
    // A defaulted extern (no `@js`) imports from `("env", <name>)` and carries its result type.
    let code = "
        extern fun now(): int;
        fun t(): int { return now(); }
    ";
    let wat = emit_hir_to_module(code);
    assert!(
        wat.contains("(import \"env\" \"now\" (func $now (result i32)))"),
        "defaulted extern should import from env with its result:\n{}",
        wat
    );
    wat::parse_str(&wat).expect("module importing a result-returning extern should assemble");
}

/// The `System` intrinsic surface (mirrors `stdlib/system/system.dream`), inlined so the print tests do not
/// depend on the full prelude being merged by the unit-test harness.
const SYSTEM_STUB: &str = "
    class System {
        @intrinsic(\"print\")
        static extern fun print<T>(value: T): void;
        @intrinsic(\"println\")
        static extern fun println<T>(value: T): void;
    }
";

/// `System` + `Time.sleep` for async tests (mirrors `stdlib/system/time.dream` + `system.dream`).
const ASYNC_STUB: &str = "
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

#[test]
fn test_hir_emission_print_int_and_println() {
    // `System.print(int)` lowers to `$print_int`; `println` adds a trailing newline (`\n` = 10) via
    // `$print_char`. Both link against the host import prelude and assemble.
    let code = format!(
        "{SYSTEM_STUB}
        fun run(): void {{
            System.print(41);
            System.println(42);
        }}"
    );
    let wat = emit_hir_to_module(&code);
    assert!(wat.contains("(call $print_int)"), "print(int) should call $print_int:\n{}", wat);
    assert!(
        wat.contains("(i32.const 10)") && wat.contains("(call $print_char)"),
        "println should append a newline via $print_char:\n{}",
        wat
    );
    wat::parse_str(&wat).expect("module printing an int should assemble");
}

#[test]
fn test_hir_emission_print_string_interns_literal() {
    // `System.print(string)` lowers to `$print_string` and interns the literal as a data segment.
    let code = format!("{SYSTEM_STUB} fun run(): void {{ System.print(\"hi\"); }}");
    let wat = emit_hir_to_module(&code);
    assert!(wat.contains("(call $print_string)"), "print(string) should call $print_string:\n{}", wat);
    assert!(wat.contains("(data "), "the string literal should be interned:\n{}", wat);
    wat::parse_str(&wat).expect("module printing a string should assemble");
}

#[test]
fn test_hir_emission_print_char() {
    let code = format!("{SYSTEM_STUB} fun run(): void {{ System.print('x'); }}");
    let wat = emit_hir_to_module(&code);
    assert!(wat.contains("(call $print_char)"), "print(char) should call $print_char:\n{}", wat);
    wat::parse_str(&wat).expect("module printing a char should assemble");
}

#[test]
fn test_hir_emission_print_bool_float_double_long() {
    // Non-`int`/`char`/`string` scalars render through their in-wasm `*_to_string` then print as a
    // string. The module bundles those formatters (+ the `true`/`false`/`-` constants) and assembles.
    let code = format!(
        "{SYSTEM_STUB}
        fun run(b: bool, f: float, d: double, l: long): void {{
            System.print(b);
            System.print(f);
            System.print(d);
            System.print(l);
        }}"
    );
    let wat = emit_hir_to_module(&code);
    for helper in ["$bool_to_string", "$float_to_string", "$double_to_string", "$long_to_string"] {
        assert!(wat.contains(&format!("(call {helper})")), "missing {helper} in print:\n{}", wat);
    }
    assert!(wat.contains("(func $bool_to_string"), "bool formatter should be defined:\n{}", wat);
    wat::parse_str(&wat).expect("module printing non-int scalars should assemble");
}

#[test]
fn test_hir_emission_print_object_routes_to_print_object() {
    // Printing an object is now covered: it lowers to `Statement::Print` over a reference type, which
    // the backend renders through the tag-dispatching `$print_object`.
    let code = format!(
        "{SYSTEM_STUB}
        class Box {{ public v: int; }}
        fun run(b: Box): void {{ System.print(b); }}"
    );
    let module = emit_hir_to_module(&code);
    assert!(module.contains("(func $run"), "an object print should be covered now:\n{}", module);
    assert!(module.contains("(call $print_object)"), "object print routes to $print_object:\n{}", module);
    assert!(module.contains("(func $Box_to_string"), "a default struct to_string is generated:\n{}", module);
    wat::parse_str(&module).expect("object-printing module should assemble");
}

#[cfg(feature = "native")]
#[test]
fn exec_print_int_and_arithmetic() {
    // Runs a real program through the MIR backend: `print` of an int literal and of a computed sum,
    // proving the host import + integer path execute end-to-end.
    let code = format!(
        "{SYSTEM_STUB}
        fun main(): void {{
            System.print(41);
            System.print(1 + 1);
        }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "412");
}

#[cfg(feature = "native")]
#[test]
fn exec_println_int_appends_newline() {
    let code = format!("{SYSTEM_STUB} fun main(): void {{ System.println(7); }}");
    assert_eq!(run_and_capture(&code, "main"), "7\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_print_string_literal() {
    // Validates the reconciled string ABI: the interned literal's data pointer is a length-prefixed
    // heap string the host reads correctly.
    let code = format!("{SYSTEM_STUB} fun main(): void {{ System.println(\"hello\"); }}");
    assert_eq!(run_and_capture(&code, "main"), "hello\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_print_bool_via_to_string() {
    // Exercises the bundled `*_to_string` runtime: `bool` renders through `$bool_to_string`, whose
    // interned "true"/"false" are printed as length-prefixed strings.
    let code = format!(
        "{SYSTEM_STUB}
        fun main(): void {{
            System.println(true);
            System.println(false);
        }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "true\nfalse\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_string_len_via_strlen() {
    // `str.size()` calls `$strlen`, an O(1) load of the length-prefixed string's length word.
    let code = format!(
        "{SYSTEM_STUB}
        fun main(): void {{
            let s: string = \"hello\";
            System.print(s.size());
        }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "5");
}

#[cfg(feature = "native")]
#[test]
fn exec_print_long_literal_via_to_string() {
    // The exact case that used to fail assembly (`123456789012` emitted as an out-of-range
    // `i32.const`): a magnitude-typed `long` literal now lowers to `i64.const` and renders via
    // `$long_to_string`.
    let code = format!("{SYSTEM_STUB} fun main(): void {{ System.println(123456789012); }}");
    assert_eq!(run_and_capture(&code, "main"), "123456789012\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_long_arithmetic_stays_i64() {
    // Exercises the i64 add path end-to-end: two `long` locals summed and printed.
    let code = format!(
        "{SYSTEM_STUB}
        fun main(): void {{
            let a: long = 100000000000;
            let b: long = 23456789012;
            System.println(a + b);
        }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "123456789012\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_print_struct_via_object_to_string() {
    // Object print end-to-end: `Point(1, 2)` allocates a tagged struct, and `$print_object` routes
    // through the generated `$Point_to_string` to render `Point { x: 1, y: 2 }`.
    let code = format!(
        "{SYSTEM_STUB}
        class Point {{ public x: int; public y: int; constructor(x: int, y: int) {{ this.x = x; this.y = y; }} }}
        fun main(): void {{ System.println(Point(1, 2)); }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "Point { x: 1, y: 2 }\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_print_nested_struct() {
    // A struct field that is itself a struct renders recursively via `$object_to_string`.
    let code = format!(
        "{SYSTEM_STUB}
        class Point {{ public x: int; public y: int; constructor(x: int, y: int) {{ this.x = x; this.y = y; }} }}
        class Line {{ public a: Point; public b: Point; constructor(a: Point, b: Point) {{ this.a = a; this.b = b; }} }}
        fun main(): void {{ System.println(Line(Point(1, 2), Point(3, 4))); }}"
    );
    assert_eq!(
        run_and_capture(&code, "main"),
        "Line { a: Point { x: 1, y: 2 }, b: Point { x: 3, y: 4 } }\n"
    );
}

#[cfg(feature = "native")]
#[test]
fn exec_print_union_variants() {
    // Union print: the tag-dispatched `$<Union>_to_string` reads the discriminant and renders the
    // active variant. Data variants render `Variant(field: value, ...)`; unit variants render bare.
    let code = format!(
        "{SYSTEM_STUB}
        enum Shape {{ Circle(radius: int), Rect(width: int, height: int), Empty }}
        fun main(): void {{
            System.println(Shape.Circle(5));
            System.println(Shape.Rect(2, 3));
            System.println(Shape.Empty);
        }}"
    );
    assert_eq!(
        run_and_capture(&code, "main"),
        "Circle(radius: 5)\nRect(width: 2, height: 3)\nEmpty\n"
    );
}

#[cfg(feature = "native")]
#[test]
fn exec_print_int_array() {
    // Array print: the element-typed `$array_to_string_t<id>` renders `[e0, e1, ...]`.
    let code = format!(
        "{SYSTEM_STUB} fun main(): void {{ let xs: int[] = [10, 20, 30]; System.println(xs); }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "[10, 20, 30]\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_print_struct_array() {
    // An array of structs renders each element via the struct's `to_string` (reference elements route
    // through `$object_to_string`).
    let code = format!(
        "{SYSTEM_STUB}
        class Point {{ public x: int; public y: int; constructor(x: int, y: int) {{ this.x = x; this.y = y; }} }}
        fun main(): void {{
            let ps: Point[] = [Point(1, 2), Point(3, 4)];
            System.println(ps);
        }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "[Point { x: 1, y: 2 }, Point { x: 3, y: 4 }]\n");
}

#[cfg(feature = "native")]
#[test]
fn exec_del_runs_at_last_release() {
    // Overwriting a reference local releases its previous occupant; at refcount zero the deep-release
    // runtime runs the object's `del()` (prints 9 here) before freeing. So `Res(1)` is released (9)
    // when `r` is reassigned, the surviving `Res(2)` prints its field (2), and finally the scope-exit
    // release of `r` runs `Res(2).del()` (9) at function return -> "929". Proves overwrite release,
    // `$release_Res` -> `$Res_del` -> `$free`, and scope-exit release all fire end-to-end.
    let code = format!(
        "{SYSTEM_STUB}
        class Res {{ public v: int;
            del() {{ System.print(9); }}
            constructor(v: int) {{ this.v = v; }}
        }}
        fun main(): void {{
            let r: Res = Res(1);
            r = Res(2);
            System.print(r.v);
        }}"
    );
    assert_eq!(run_and_capture_rc(&code, "main"), "929");
}

#[test]
fn exec_container_store_retains_no_double_free() {
    // Storing a borrowed reference into a container field retains it, so the field and the source
    // local each own a count. At scope exit both `a` and `b` are released: releasing `a` runs its
    // `del()` (1) and deep-releases `a.next` (dropping `b` to 1), then releasing `b` runs its `del()`
    // (1) and frees it. Each object is destroyed exactly once -> "011". Without the container retain
    // this double-frees `b`.
    let code = format!(
        "{SYSTEM_STUB}
        class Node {{ public next: Node?;
            del() {{ System.print(1); }}
            constructor() {{ }}
        }}
        fun main(): void {{
            let a: Node = Node();
            let b: Node = Node();
            a.next = b;
            System.print(0);
        }}"
    );
    assert_eq!(run_and_capture_rc(&code, "main"), "011");
}

#[test]
fn exec_returned_value_transfers_ownership() {
    // `make()` returns an owned local; its `+1` transfers to the caller instead of being released at
    // `make`'s scope exit (which would run `del()` early and hand back a dangling pointer). So `y.v`
    // reads 5, and the object's single `del()` (7) fires only at `main`'s scope exit -> "57".
    let code = format!(
        "{SYSTEM_STUB}
        class R {{ public v: int;
            del() {{ System.print(7); }}
            constructor(v: int) {{ this.v = v; }}
        }}
        fun make(): R {{
            let x: R = R(5);
            return x;
        }}
        fun main(): void {{
            let y: R = make();
            System.print(y.v);
        }}"
    );
    assert_eq!(run_and_capture_rc(&code, "main"), "57");
}

/// Hand-builds a two-function MIR that takes `add` as a first-class value and calls it indirectly:
/// `fun main() { let f = add; print(f(2, 3)); }`. The analyzer does not yet emit function values, so
/// this exercises the backend (FuncRef -> table index, function table + signature, `call_indirect`)
/// directly. Returns the interner alongside so its `TypeId`s stay valid.
fn indirect_call_demo() -> (crate::mir::Mir, crate::types::TypeInterner) {
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{BinOp, Callee, Const, Mir, Operand, Place, Rvalue, Statement, Terminator};
    use crate::types::{DefId, TypeInterner};

    let mut i = TypeInterner::new();
    let int = i.int();
    let void = i.void();
    let functy = i.func(vec![int, int], int);
    let add_def = DefId(10);

    let mut ab = FunctionBuilder::new("add", int);
    ab.set_def(add_def, vec![]);
    let a = ab.new_param(int, Some("a".into()));
    let b = ab.new_param(int, Some("b".into()));
    let t = ab.new_temp(int);
    ab.assign(
        Place::Local(t),
        Rvalue::Binary(BinOp::Add, Operand::Copy(Place::Local(a)), Operand::Copy(Place::Local(b))),
    );
    ab.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));

    let mut mb = FunctionBuilder::new("main", void);
    mb.set_def(DefId(11), vec![]);
    let f = mb.new_local(functy, Some("f".into()));
    let r = mb.new_local(int, Some("r".into()));
    mb.assign(Place::Local(f), Rvalue::FuncRef(Callee { def: add_def, args: vec![], ret: int }));
    mb.assign(
        Place::Local(r),
        Rvalue::IndirectCall {
            target: Operand::Copy(Place::Local(f)),
            args: vec![Operand::Const(Const::Int(2)), Operand::Const(Const::Int(3))],
        },
    );
    mb.push(Statement::Print { arg: Operand::Copy(Place::Local(r)), ty: int, newline: false });
    mb.terminate(Terminator::Return(None));

    (Mir { functions: vec![ab.finish(), mb.finish()], ..Default::default() }, i)
}

#[test]
fn test_indirect_call_emits_table_and_signature() {
    let (mir, interner) = indirect_call_demo();
    let wat = crate::mir::emit::emit_module(&mir, &interner, false);
    assert!(wat.contains("(table $__ft 2 funcref)"), "function table missing:\n{}", wat);
    assert!(wat.contains("(elem (i32.const 0) $add $main)"), "elem section missing:\n{}", wat);
    assert!(wat.contains("(type $sig_i32_i32__i32"), "call_indirect signature missing:\n{}", wat);
    assert!(wat.contains("(call_indirect $__ft (type $sig_i32_i32__i32))"), "indirect call missing:\n{}", wat);
    assert!(
        wat.contains("(export \"__indirect_function_table\" (table $__ft))"),
        "table export missing:\n{}",
        wat
    );
}

#[cfg(feature = "native")]
#[test]
fn exec_indirect_call_through_function_table() {
    // End-to-end: `f(2, 3)` dispatches through the table to `add`, printing `5`.
    let (mir, interner) = indirect_call_demo();
    let wat = crate::mir::emit::emit_module(&mir, &interner, false);
    assert_eq!(run_wat(&wat, "main"), "5");
}

#[test]
fn test_hir_emission_first_class_function() {
    // A bare function name is a value (`Binding::Func`), and calling a function-typed local emits an
    // `IndirectCall` — both are now HIR-representable, so `main` stays in coverage.
    let code = format!(
        "{SYSTEM_STUB}
        fun add(a: int, b: int): int {{ return a + b; }}
        fun main(): void {{ let f = add; System.print(f(2, 3)); }}"
    );
    let wat = emit_hir_to_module(&code);
    assert!(wat.contains("(call_indirect $__ft"), "indirect call not emitted:\n{}", wat);
    assert!(wat.contains("funcref"), "function value not emitted:\n{}", wat);
}

#[cfg(feature = "native")]
#[test]
fn exec_first_class_function_from_source() {
    // Full pipeline: source with a first-class function -> analyzer HIR -> MIR -> table dispatch.
    let code = format!(
        "{SYSTEM_STUB}
        fun add(a: int, b: int): int {{ return a + b; }}
        fun main(): void {{ let f = add; System.print(f(2, 3)); }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "5");
}

#[test]
fn func_value_argument_is_not_reference_counted() {
    // Documents how memory is managed when a function is passed as an argument: it isn't. A
    // `fun(...)` value is a plain `i32` table index, not a heap reference, so the RC pass never
    // retains or releases it. A `string` bound alongside it in the same scope still gets its normal
    // reference-counting treatment — proving the distinction is real, not that RC is globally off.
    let code = format!(
        "{SYSTEM_STUB}
        fun twice(x: int): int {{ return x * 2; }}
        fun apply(f: fun(int): int, s: string): int {{ return f(3); }}
        fun main(): void {{
            let g: fun(int): int = twice;
            let s: string = \"hi\";
            let r: int = apply(g, s);
        }}"
    );

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
    let mut mir = crate::mir::lower::lower_program(&hir, interner);
    use crate::mir::passes::MirPass;
    for f in &mut mir.functions {
        crate::mir::passes::RcInsertion.run(f, interner);
    }

    use crate::mir::{Operand, Place, Statement};
    let main = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main should be lowered");

    let mut func_value_rc = 0usize;
    let mut reference_rc = 0usize;
    for block in &main.blocks {
        for stmt in &block.stmts {
            let op = match stmt {
                Statement::Retain(o) | Statement::Release(o) => o,
                _ => continue,
            };
            if let Operand::Copy(Place::Local(l)) = op {
                let ty = main.locals[l.0 as usize].ty;
                if matches!(
                    interner.kind(interner.strip_nullable(ty)),
                    crate::types::TyKind::Func(_, _)
                ) {
                    func_value_rc += 1;
                } else if interner.is_reference(ty) {
                    reference_rc += 1;
                }
            }
        }
    }

    assert_eq!(
        func_value_rc, 0,
        "a function value is a scalar table index and must never be retained/released:\n{:#?}",
        main
    );
    assert!(
        reference_rc > 0,
        "the string local should still be reference-counted:\n{:#?}",
        main
    );
}

#[cfg(feature = "native")]
#[test]
fn exec_print_escapes_in_string_literal() {
    // The literal-unescaping in HIR emission turns `\t` into a real tab and drops the source quotes.
    let code = format!("{SYSTEM_STUB} fun main(): void {{ System.print(\"a\\tb\"); }}");
    assert_eq!(run_and_capture(&code, "main"), "a\tb");
}

#[test]
fn test_hir_emission_generic_function_instances() {
    // A generic free function is emitted once per monomorphization: `id(5)` and `id(true)` produce
    // two instance bodies with distinct symbols, and each call site resolves to its instance.
    let code = "
        fun id<T>(x: T): T { return x; }
        fun driver(): int { let a: int = id(5); let b: bool = id(true); return a; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 3, "two id instances + driver should be emitted:\n{}", wat);
    let instances = wat.matches("(func $id__").count();
    assert_eq!(instances, 2, "each monomorphization gets its own symbol:\n{}", wat);
    assert_eq!(
        wat.matches("(call $id__").count(),
        2,
        "each generic call site should resolve to an instance symbol:\n{}",
        wat
    );
    assert!(
        !wat.contains("(call $def"),
        "no generic call should fall back to a def{{N}} placeholder:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_string_literal() {
    // A string literal resolves to its interned data pointer. The runtime constants are interned
    // first (`true`/`false`/`-` then the object-protocol `null`/`<object>`/`[`/`]`/`, `), so the
    // user's `"hi"` follows them at 1204 (each block carries a 4-byte length prefix, no NUL).
    let code = "fun greet(): string { return \"hi\"; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the string-returning function should be emitted as HIR");
    assert!(wat.contains("(func $greet"), "missing emitted function:\n{}", wat);
    assert!(wat.contains("(i32.const 1204)"), "string literal should resolve to its data pointer:\n{}", wat);
}

#[test]
fn test_hir_emission_field_read_and_constructor() {
    // A struct-field read and a (non-generic) constructor are both representable; field indexing is
    // resolved from the struct layout and `new` resolves the struct's `DefId`.
    let code = "
        class Point { public x: int; public y: int; constructor(x: int, y: int) { this.x = x; this.y = y; } }
        fun getx(p: Point): int { return p.x; }
        fun make(): Point { return Point(1, 2); }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 3, "the field-read, constructor, and constructor-body functions should be emitted");
    assert!(wat.contains("(func $getx"), "missing field-read function:\n{}", wat);
    assert!(wat.contains("(func $make"), "missing constructor function:\n{}", wat);
    // `p.x` (field 0) lowers to a real load now that the layout is threaded through.
    assert!(wat.contains("(i32.load)"), "field read should lower to a load:\n{}", wat);
    // `Point(1, 2)` allocates and initializes fields.
    assert!(wat.contains("(call $malloc)"), "constructor should allocate:\n{}", wat);
}

#[test]
fn test_hir_emission_field_assignment() {
    // Writing through a struct field lowers to an `Assign` with a `Field` place.
    let code = "
        class Counter { public n: int; }
        fun bump(c: Counter): void { c.n = c.n + 1; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the field-assignment function should be emitted");
    assert!(wat.contains("(func $bump"), "missing field-assignment function:\n{}", wat);
    // `c.n = ...` lowers to a real store through the field address.
    assert!(wat.contains("(i32.store)"), "field write should lower to a store:\n{}", wat);
}

#[test]
fn test_hir_emission_index_assignment() {
    // Indexed assignment lowers to an `Assign` with an `Index` place.
    let code = "fun setfirst(xs: int[], v: int): void { xs[0] = v; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the index-assignment function should be emitted");
    assert!(wat.contains("(func $setfirst"), "missing index-assignment function:\n{}", wat);
    // `xs[0] = v` computes the element address (base + 4 + i*stride) and stores.
    assert!(wat.contains("(i32.store)"), "index write should lower to a store:\n{}", wat);
}

#[test]
fn test_hir_emission_enum_value() {
    // An enum-member reference resolves to its constant integer value.
    let code = "
        enum Color { Red, Green, Blue }
        fun pick(): Color { return Color.Green; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the enum-returning function should be emitted");
    assert!(wat.contains("(func $pick"), "missing enum function:\n{}", wat);
    // `Color.Green` is the second member, value 1.
    assert!(wat.contains("i32.const 1"), "missing enum constant:\n{}", wat);
}

#[test]
fn test_hir_emission_method_body_and_instance_call() {
    // A method body (with a `this` receiver and a field read) is emitted under its mangled name,
    // and a resolved instance-method call lowers to a `MethodCall`.
    let code = "
        class Box { public v: int; public fun get(): int { return this.v; } }
        fun use_box(b: Box): int { return b.get(); }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both the method body and its caller should be emitted:\n{}", wat);
    assert!(wat.contains("(func $Box_get"), "missing emitted method body:\n{}", wat);
    assert!(wat.contains("(func $use_box"), "missing instance-call function:\n{}", wat);
    assert!(wat.contains("(call $Box_get"), "instance call should dispatch to the method:\n{}", wat);
}

#[test]
fn test_hir_emission_static_call() {
    // A (non-generic) static method is a free function under its mangled `{Type}_{method}` name;
    // calling it lowers to a direct `Call`.
    let code = "
        class M { public static fun id(n: int): int { return n; } }
        fun use_static(): int { return M.id(7); }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both the static method and its caller should be emitted:\n{}", wat);
    assert!(wat.contains("(func $M_id"), "missing emitted static method:\n{}", wat);
    assert!(wat.contains("(call $M_id"), "static call should dispatch to the method:\n{}", wat);
}

#[test]
fn test_hir_emission_global_read_and_write() {
    // A module-global resolves to a `Global` binding for both reads and assignments.
    let code = "
        let counter: int = 0;
        fun tick(): int { counter = counter + 1; return counter; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the global-using function should be emitted:\n{}", wat);
    assert!(wat.contains("global.get $g0"), "missing global read:\n{}", wat);
    assert!(wat.contains("global.set $g0"), "missing global write:\n{}", wat);
}

#[test]
fn test_hir_emission_union_construction() {
    // Constructing a (non-generic) discriminated-union variant lowers to a `UnionNew`. `Shape` has
    // only primitive payloads, so it is inferred as a *value union*: constructed inline into the
    // return slot (its first word is the discriminant) rather than heap-allocated.
    let code = "
        enum Shape { Circle(radius: int), Empty }
        fun mk(): Shape { return Shape.Circle(2); }
        fun nil(): Shape { return Shape.Empty; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both union constructors should be emitted:\n{}", wat);
    assert!(wat.contains("(func $mk"), "missing data-variant constructor:\n{}", wat);
    assert!(wat.contains("(func $nil"), "missing unit-variant constructor:\n{}", wat);
    // A value union is written inline (no `$malloc`), and its first word is the variant discriminant.
    assert!(
        !wat.contains("(call $malloc)"),
        "value-union construction should not allocate on the heap:\n{}",
        wat
    );
    assert!(
        wat.contains(";; discriminant"),
        "union block should store its discriminant:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_switch_statement() {
    // A `switch` with single-label cases and a `default` lowers to `HStmt::Switch`.
    let code = "
        fun classify(n: int): int {
            let r: int = 0;
            switch (n) {
                case 1: r = 10;
                case 2: r = 20;
                default: r = 30;
            }
            return r;
        }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the switch function should be emitted:\n{}", wat);
    assert!(wat.contains("(func $classify"), "missing switch function:\n{}", wat);
}

#[test]
fn test_hir_emission_switch_statement_with_variant_binding() {
    // A statement-position pattern `switch` lowers to `HStmt::Switch`; a variant pattern binds its
    // payload to fresh locals that the arm body resolves.
    let code = "
        enum Shape { Circle(radius: int), Empty }
        fun describe(s: Shape): int {
            let r: int = 0;
            switch (s) {
                Circle(rad) => { r = rad; }
                Empty => { r = 0; }
            }
            return r;
        }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the switch function should be emitted:\n{}", wat);
    assert!(wat.contains("(func $describe"), "missing switch function:\n{}", wat);
}

#[test]
fn test_hir_emission_len_builtin() {
    // `arr.size()` reads the array's stored length word; `str.size()` calls the runtime `$strlen`
    // (both are O(1) loads now that strings are length-prefixed heap objects).
    let code = "
        fun count(xs: int[]): int { return xs.size(); }
        fun slen(s: string): int { return s.size(); }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both size functions should be emitted:\n{}", wat);
    assert!(wat.contains("(func $count"), "missing array-len function:\n{}", wat);
    assert!(wat.contains("(func $slen"), "missing string-len function:\n{}", wat);
    assert!(wat.contains("(call $strlen)"), "string len should use $strlen:\n{}", wat);
    // A full module (with the string runtime) must assemble, proving `$strlen` is provided.
    let module = emit_hir_to_module(code);
    wat::parse_str(&module).expect("module using $strlen should assemble");
}

#[test]
fn test_hir_emission_switch_expression() {
    // A value-position `switch` desugars to a result temp + `Switch`, read back as the switch value.
    let code = "
        enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }
        fun area(s: Shape): int {
            return switch (s) {
                Circle(r)  => r * r,
                Rect(w, h) => w * h,
                Empty      => 0,
            };
        }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the switch-expression function should be emitted:\n{}", wat);
    assert!(wat.contains("(func $area"), "missing switch-expression function:\n{}", wat);
}

#[test]
fn test_hir_emission_async_await() {
    // Async bodies emit with `Await` nodes; an async call carries a `Future` return type.
    let code = "
        async fun delay(): void { }
        async fun work(n: int): int { await delay(); return n; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 2, "both async functions should be emitted:\n{}", wat);
    assert!(wat.contains("(func $work"), "missing async function:\n{}", wat);
}

#[test]
fn test_async_emits_scheduler_runtime_and_poll() {
    let code = format!(
        "{ASYNC_STUB}
        async fun delay(): void {{ await Time.sleep(0); }}
        async fun main(): void {{ await delay(); }}"
    );
    let wat = emit_hir_to_module(&code);
    assert!(wat.contains("(func $dream_run_loop"), "scheduler missing:\n{}", wat);
    assert!(wat.contains("(func $poll_delay"), "poll fn missing:\n{}", wat);
    assert!(wat.contains("call $dream_new_future"), "constructor missing:\n{}", wat);
    assert!(wat.contains("call $dream_await"), "suspend missing:\n{}", wat);
    assert!(wat.contains("(export \"main\")"), "async main wrapper missing:\n{}", wat);
}

#[cfg(feature = "native")]
#[test]
fn exec_async_sleep_and_await() {
    let code = format!(
        "{ASYNC_STUB}
        async fun get(): int {{
            await Time.sleep(0);
            return 42;
        }}
        async fun main(): void {{
            let v = await get();
            System.print(v);
        }}"
    );
    assert_eq!(run_and_capture(&code, "main"), "42");
}

#[test]
fn test_analyze_valid_types() {
    let code = "fun main(): void { let x: int = 5; let y: float = 3.14; let z: string = \"hello\"; let b: bool = true; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_type_mismatch() {
    let code = "fun main(): void { let x: int = \"hello\"; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("cannot convert from int to string")));
}

#[test]
fn test_analyze_new_integer_widening_ok() {
    // The full widening lattice: narrower numeric values flow into wider numeric targets without
    // an explicit cast.
    let code = "fun main(): void {
        let l: long = 5;
        let l2: long = 7u;
        let ul: ulong = 9u;
        let d: double = 9000000000L;
        let i: int = 200b;
        let f: float = 3000000000u;
    }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_new_integer_narrowing_requires_cast() {
    // Assigning a `long` to an `int` is a narrowing conversion and must be rejected without a cast.
    let code = "fun main(): void { let x: int = 5L; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_analyze_new_integer_explicit_casts_ok() {
    // Explicit casts permit narrowing and same-width sign changes between the numeric types.
    let code = "fun main(): void {
        let a: int = (int)9000000000L;
        let b: byte = (byte)511;
        let c: uint = (uint)5;
        let e: long = (long)4000000000u;
    }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_unary_minus_allows_all_numeric_types() {
    // Regression test: unary +/- used to be rejected for `long`/`uint`/`ulong`/`byte`, even though
    // the MIR backend (const-fold + codegen) already handled them like `int`/`float`/`double`.
    let code = "fun main(): void {
        let a: long = -15L;
        let b: uint = -3u;
        let c: ulong = -7ul;
        let d: byte = -(byte)1;
        let e: int = -42;
        let f: float = -1.5f;
        let g: double = -2.5d;
        let h = +5;
    }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_unary_minus_rejects_non_numeric_types() {
    let code = "fun main(): void { let x = -\"hello\"; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("unary +/- requires a numeric type")));
}

#[test]
fn test_analyze_undefined_variable() {
    let code = "fun main(): void { let x = y + 5; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("variable y does not exist")));
}

#[test]
fn test_analyze_array_operations() {
    let code = "
        fun main(): void { 
            let arr: int[] = [1, 2, 3]; 
            let x: int = arr[0];
            arr[1] = 5;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_invalid_array_operations() {
    let code = "
        fun main(): void { 
            let arr: int[] = [1, 2, 3]; 
            arr[\"hello\"] = 5; // Invalid index type
            let x: int = 5;
            x[0] = 1; // Indexing non-array
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Array index must be of type int")));
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Cannot index into non-array type int")));
}

#[test]
fn test_analyze_async_await_valid() {
    // Calling an async fun yields `Future<T>`; awaiting it (at a statement position) yields `T`.
    let code = "
        async fun delay(): void { }
        async fun work(n: int): int { await delay(); return n * 2; }
        async fun main(): void {
            let h = work(3);
            let v = await h;
            let w = await work(4);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_await_outside_async() {
    let code = "async fun delay(): int { return 1; } fun main(): void { let x = await delay(); }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics.diagnostics.iter().any(|d| d
        .message
        .contains("can only be used inside an 'async' function")));
}

#[test]
fn test_analyze_await_in_subexpression_rejected() {
    // v1 restricts `await` to top-level statement positions.
    let code = "
        async fun delay(): void { }
        async fun work(n: int): int { await delay(); return n; }
        async fun main(): void { let x = await work(1) + 1; }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("top-level statement")));
}

#[test]
fn test_analyze_await_non_future_rejected() {
    let code = "async fun main(): void { let x = await 5; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_unresolved_identifier_does_not_cascade() {
    // A single unresolved identifier should report exactly one error: the poison/`Unknown` type it
    // produces unifies with everything, so the downstream `+`, the `: int` annotation, the call
    // argument, and the array index must NOT each add their own follow-on diagnostic.
    let code = "
        fun takes_int(n: int): int { return n; }
        fun main(): void {
            let a: int = missing + 1;
            let b: int = takes_int(missing);
            let arr: int[] = [1, 2, 3];
            let c: int = arr[missing];
        }
    ";
    let diagnostics = analyze_code(code);
    let errors: Vec<&str> = diagnostics
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    // Three uses of `missing`, so three "does not exist" errors -- and nothing else.
    let undefined = errors
        .iter()
        .filter(|m| m.contains("missing does not exist"))
        .count();
    assert_eq!(
        undefined, 3,
        "expected 3 undefined-identifier errors, got: {:?}",
        errors
    );
    assert_eq!(
        errors.len(),
        3,
        "poison type should suppress cascading errors; got: {:?}",
        errors
    );
}

#[test]
fn test_unknown_call_result_does_not_cascade() {
    // Calling an unknown function poisons the result; the inferred variable is poison too, so
    // using it must not pile on more errors.
    let code = "
        fun main(): void {
            let x = nope();
            let y: int = x + 1;
            let z: bool = x;
        }
    ";
    let diagnostics = analyze_code(code);
    let errors: Vec<&str> = diagnostics
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "only the unknown-function error should be reported; got: {:?}",
        errors
    );
}

#[test]
fn test_analyze_union_switch_ok() {
    let code = "
        enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }
        fun area(s: Shape): int {
            return switch (s) {
                Circle(r)  => r * r,
                Rect(w, h) => w * h,
                Empty      => 0,
            };
        }
        fun main(): void { let a: int = area(Shape.Circle(2)); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_union_switch_non_exhaustive() {
    let code = "
        enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }
        fun area(s: Shape): int {
            return switch (s) {
                Circle(r)  => r * r,
                Rect(w, h) => w * h,
            };
        }
        fun main(): void { let a: int = area(Shape.Empty); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Non-exhaustive switch") && d.message.contains("Empty")));
}

#[test]
fn test_analyze_union_variant_arity_mismatch() {
    let code = "
        enum Shape { Circle(radius: int), Empty }
        fun main(): void { let s: Shape = Shape.Circle(1, 2); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("expects 1 argument")));
}

#[test]
fn test_analyze_generic_union_inference() {
    let code = "
        enum Option<T> { Some(value: T), None }
        fun main(): void {
            let o = Option.Some(42);
            let n: Option<int> = Option.None;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_switch_expression_arm_type_mismatch() {
    let code = "
        enum Shape { Circle(radius: int), Empty }
        fun f(s: Shape): int {
            return switch (s) {
                Circle(r) => r,
                Empty     => \"oops\",
            };
        }
        fun main(): void { let x: int = f(Shape.Empty); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

// -- Class indexer (`obj[i]` / `obj[i] = v`) and enumerator (`for (let x in obj)`) --

#[test]
fn test_class_indexer_get_set_ok() {
    // A class with an instance `get(index): T` (non-void) and `set(index, value)` is indexable.
    let code = "
        class Cell {
            v: int;
            constructor() { this.v = 0; }
            public fun get(index: int): int { return this.v + index; }
            public fun set(index: int, value: int): void { this.v = value; }
        }
        fun main(): void {
            let c = Cell();
            c[1] = 5;
            let x: int = c[2];
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_class_indexer_void_get_is_not_an_indexer() {
    // A `get` returning `void` is a normal method, not an indexer: `obj[i]` errors.
    let code = "
        class Box {
            v: int;
            constructor() { this.v = 0; }
            public fun get(index: int): void { }
        }
        fun main(): void {
            let b = Box();
            let x = b[0];
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("must return a value")));
}

#[test]
fn test_class_void_get_still_callable_as_method() {
    // Defining a void `get` must NOT break calling it directly as an ordinary method.
    let code = "
        class Box {
            v: int;
            constructor() { this.v = 0; }
            public fun get(index: int): void { }
        }
        fun main(): void {
            let b = Box();
            b.get(0);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_class_indexer_static_get_is_not_an_indexer() {
    // A `static get` has no receiver, so it can't be an instance indexer: `obj[i]` errors.
    let code = "
        class Box {
            v: int;
            constructor() { this.v = 0; }
            public static fun get(index: int): int { return index; }
        }
        fun main(): void {
            let b = Box();
            let x = b[0];
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("non-static")));
}

#[test]
fn test_class_indexer_async_get_is_not_an_indexer() {
    // An `async get` yields a `Future`, so it can't be a (synchronous) indexer: `obj[i]` errors.
    let code = "
        class Box {
            v: int;
            constructor() { this.v = 0; }
            public async fun get(index: int): int { return index; }
        }
        fun main(): void {
            let b = Box();
            let x = b[0];
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("cannot be async")));
}

// -- TypeScript-style property accessors (`get prop()` / `set prop(v)`) --

#[test]
fn test_property_getter_ok() {
    // A well-formed getter/setter pair is read/written via dot access, not brackets.
    let code = "
        class Box {
            v: int;
            constructor() { this.v = 0; }
            public get value(): int { return this.v; }
            public set value(x: int) { this.v = x; }
        }
        fun main(): void {
            let b = Box();
            b.value = 5;
            let x: int = b.value;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_static_getter_and_setter_ok() {
    // Static accessors are read/written through the type itself: `Counter.count` calls the static
    // getter and `Counter.count = v` calls the static setter (no instance receiver).
    let code = "
        class Counter {
            public static get count(): int { return 42; }
            public static set count(x: int) { }
        }
        fun main(): void {
            Counter.count = 5;
            let n: int = Counter.count;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_static_getter_type_mismatch_is_reported() {
    // A static getter is still type-checked: assigning its `int` result to a `string` errors.
    let code = "
        class Box {
            public static get value(): int { return 0; }
        }
        fun main(): void {
            let s: string = Box.value;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_array_size_builtin_ok() {
    // `arr.size()` is the builtin element-count method on arrays, typed `int` (the same `size()`
    // the stdlib collections expose). Cross-collection consistency is covered by the
    // `size_consistent` e2e case.
    let code = "
        fun main(): void {
            let a = [10, 20, 30];
            let n: int = a.size();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_async_accessor_is_rejected() {
    // An `async` getter would yield a `Future` instead of the property value, so it is rejected.
    let code = "
        class Box {
            v: int;
            constructor() { this.v = 0; }
            public async get value(): int { return this.v; }
        }
        fun main(): void {
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("cannot be 'async'")));
}

#[test]
fn test_class_foreach_with_option_enumerator_ok() {
    // The full enumerator protocol: `iterator()` returns an object whose `next(): Option<T>`
    // yields elements. `break`/`continue` are valid in the body.
    let code = "
        enum Option<T> { Some(value: T), None }
        class RangeIter {
            cur: int;
            end: int;
            constructor(s: int, e: int) { this.cur = s; this.end = e; }
            public fun next(): Option<int> {
                if (this.cur >= this.end) { return Option.None; }
                let v = this.cur;
                this.cur = this.cur + 1;
                return Option.Some(v);
            }
        }
        class Range {
            start: int;
            end: int;
            constructor(s: int, e: int) { this.start = s; this.end = e; }
            public fun iterator(): RangeIter { return RangeIter(this.start, this.end); }
        }
        fun main(): void {
            let total = 0;
            for (let x in Range(0, 5)) {
                if (x == 2) { continue; }
                if (x == 4) { break; }
                total = total + x;
            }
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_class_foreach_next_not_option_errors() {
    // `next()` must return `Option<T>`; a `next()` returning a plain value is rejected.
    let code = "
        class NumIter {
            n: int;
            constructor() { this.n = 0; }
            public fun next(): int { return 0; }
        }
        class Nums {
            constructor() { }
            public fun iterator(): NumIter { return NumIter(); }
        }
        fun main(): void {
            for (let x in Nums()) { }
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("next()' to return Option")));
}

#[test]
fn test_class_foreach_missing_iterator_errors() {
    // A class without an `iterator()` method cannot be iterated with `for..in`.
    let code = "
        class Plain {
            v: int;
            constructor() { this.v = 0; }
        }
        fun main(): void {
            for (let x in Plain()) { }
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("iterator()")));
}

#[test]
fn test_interface_implemented_ok() {
    // A class providing every interface method with a matching signature analyzes cleanly, and a
    // concrete value flows into an interface-typed local via an implicit upcast, then dispatches.
    let code = "
        interface Animal {
            fun speak(): string;
            fun legs(): int;
        }
        class Cat : Animal {
            public fun speak(): string { return \"meow\"; }
            public fun legs(): int { return 4; }
        }
        fun run(): string {
            let c = Cat();
            let a: Animal = c;
            return a.speak();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_interface_missing_method_errors() {
    // Declaring `: Animal` obliges the class to implement every method of the interface.
    let code = "
        interface Animal {
            fun speak(): string;
            fun legs(): int;
        }
        class Cat : Animal {
            public fun speak(): string { return \"meow\"; }
        }
        fun main(): void { let c = Cat(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not implement method")
            && d.message.contains("legs")));
}

#[test]
fn test_interface_cannot_be_instantiated() {
    let code = "
        interface Animal { fun speak(): string; }
        fun main(): void { let a = Animal(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("instantiate interface")));
}

#[test]
fn test_interface_call_emits_dynamic_dispatch() {
    // A method call on an interface-typed receiver lowers to a `(call $__iface_dispatch_*)`
    // trampoline rather than a static call to a concrete method.
    let code = "
        interface Animal { fun speak(): string; }
        class Cat : Animal { public fun speak(): string { return \"meow\"; } }
        fun describe(a: Animal): string { return a.speak(); }
        fun run(): string { return describe(Cat()); }
    ";
    let (wat, _) = emit_hir_to_wat(code);
    assert!(
        wat.contains("$__iface_dispatch_"),
        "interface call should dispatch through a trampoline:\n{}",
        wat
    );
}

#[test]
fn test_generic_interface_monomorphized_ok() {
    // A generic class implementing a generic interface analyzes cleanly, and a call on the
    // monomorphized interface type dispatches dynamically.
    let code = "
        interface Container<T> {
            fun get(): T;
            fun size(): int;
        }
        class Box<T> : Container<T> {
            public value: T;
            constructor(value: T) { this.value = value; }
            public fun get(): T { return this.value; }
            public fun size(): int { return 1; }
        }
        fun describe(c: Container<int>): int { return c.get(); }
        fun run(): int {
            let b = Box<int>(7);
            return describe(b);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_generic_interface_signature_mismatch_errors() {
    // The class's monomorphized method must match the interface's monomorphized signature.
    let code = "
        interface Container<T> {
            fun get(): T;
        }
        class Box<T> : Container<T> {
            public value: T;
            public fun get(): int { return 0; }
        }
        fun run(): int {
            let b = Box<string>(\"x\");
            return b.size();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not match the signature")
            || d.message.contains("does not implement method")));
}

#[test]
fn test_async_interface_method_ok() {
    // An async interface method implemented by an async class method analyzes cleanly; calling it
    // through an interface-typed receiver yields an awaitable `Future`.
    let code = "
        interface Fetcher { async fun fetch(): int; }
        class Remote : Fetcher {
            public async fun fetch(): int { return 1; }
        }
        async fun run(f: Fetcher): int { return await f.fetch(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_async_interface_method_requires_async_impl() {
    // A sync class method cannot satisfy an async interface method (they compile to different
    // shapes), so the implements check must reject it.
    let code = "
        interface Fetcher { async fun fetch(): int; }
        class Remote : Fetcher {
            public fun fetch(): int { return 1; }
        }
        fun main(): void { let r = Remote(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not match the signature")));
}

#[test]
fn test_is_binding_not_visible_outside_branch() {
    // The `is`-with-binding local is scoped to the taken branch; referencing it afterwards is an
    // error.
    let code = "
        fun f(o: object): int {
            if (o is int a) { return a; }
            return a;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not exist")));
}

#[test]
fn test_default_param_call_with_and_without_optional_arg() {
    // A trailing default parameter may be supplied or omitted; both calls are well-typed.
    let code = "
        fun greet(name: string, times: int = 1): void {}
        fun main(): void {
            greet(\"hi\");
            greet(\"hi\", 2);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_default_param_missing_required_arg_errors() {
    // The leading required parameter still must be supplied: `greet()` provides fewer than the
    // required count and is an error.
    let code = "
        fun greet(name: string, times: int = 1): void {}
        fun main(): void {
            greet();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_default_param_too_many_args_errors() {
    // Supplying more than the total parameter count is still an arity error, reported with the
    // range message.
    let code = "
        fun greet(name: string, times: int = 1): void {}
        fun main(): void {
            greet(\"hi\", 1, 2);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("between 1 and 2 arguments")));
}

#[test]
fn test_default_param_after_required_used_in_call() {
    // The default's value is substituted, so a numeric default type-checks against its declared
    // parameter type without error.
    let code = "
        fun scale(base: int, factor: int = 2): int { return base * factor; }
        fun main(): void {
            let a: int = scale(5);
            let b: int = scale(5, 3);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_default_param_rejected_after_required_at_analysis() {
    // The parser reports a required parameter following a defaulted one; analysis surfaces it too.
    let code = "
        fun bad(x: int = 1, y: int): void {}
        fun main(): void {}
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}
