//! HIR->MIR->WAT emission and native execution tests (`test_hir_emission_*`, `exec_*`,
//! first-class/indirect calls, RC/release runtime, async emission). See `harness` for helpers.

use super::super::*;
use super::harness::*;
use crate::syntax::lexer::Lexer;
use crate::syntax::parser::Parser;
use pretty_assertions::assert_eq;

#[test]
fn test_hir_emission_arithmetic_function() {
    // A plain free function over arithmetic on parameters is fully representable in HIR, so the
    // analyzer emits it and it survives the whole new backend pipeline.
    let (wat, count) = emit_hir_to_wat("fun add(a: int, b: int): int { return a + b; }");
    assert_eq!(
        count, 1,
        "the single free function should be emitted as HIR"
    );
    assert!(
        wat.contains("(func $add"),
        "missing emitted function:\n{}",
        wat
    );
    assert!(wat.contains("i32.add"), "missing arithmetic:\n{}", wat);
}

#[test]
fn test_hir_emission_locals_and_assignment() {
    // `let` + assignment + return over locals: each statement is supported, so the function emits.
    let code = "fun calc(n: int): int { let x: int = n; let y: int = x + 1; y = y + n; return y; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1);
    assert!(
        wat.contains("(func $calc"),
        "missing emitted function:\n{}",
        wat
    );
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
    assert_eq!(
        count, 1,
        "only the fully-supported function should be emitted"
    );
}

#[test]
fn test_hir_emission_while_loop() {
    // `while` over locals is now fully representable; the whole function survives the pipeline and
    // its CFG is emitted via the block-dispatch loop.
    let code = "fun count(n: int): int { let s: int = 0; while (s < n) { s = s + 1; } return s; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the while function should be emitted as HIR");
    assert!(
        wat.contains("(func $count"),
        "missing emitted function:\n{}",
        wat
    );
    assert!(
        wat.contains("i32.lt_s"),
        "missing loop comparison:\n{}",
        wat
    );
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
    assert_eq!(
        count, 1,
        "the if/else-if/else function should be emitted as HIR"
    );
    assert!(
        wat.contains("(func $classify"),
        "missing emitted function:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $sum"),
        "missing emitted function:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $total"),
        "missing emitted function:\n{}",
        wat
    );
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
    assert_eq!(
        count, 1,
        "the logical/ternary function should be emitted as HIR"
    );
    assert!(
        wat.contains("(func $pick"),
        "missing emitted function:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_coalesce() {
    // `lhs ?? rhs` lowers to a null-test branch joining into one temp.
    let code = "fun or_default(x: string?): string { return x ?? \"d\"; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the coalesce function should be emitted as HIR");
    assert!(
        wat.contains("(func $or_default"),
        "missing emitted function:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_cast() {
    // A numeric widening cast lowers to a concrete conversion instruction.
    let code = "fun widen(x: int): double { return (double)x; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the cast function should be emitted as HIR");
    assert!(
        wat.contains("f64.convert_i32_s"),
        "missing widening cast:\n{}",
        wat
    );
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
    assert_eq!(
        count, 2,
        "both the index and array-literal functions should be emitted"
    );
    assert!(
        wat.contains("(func $first"),
        "missing index function:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $make"),
        "missing array-literal function:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $malloc)"),
        "array literal should allocate:\n{}",
        wat
    );
}

#[test]
fn test_empty_array_literal_infers_from_context() {
    // An untyped `[]` resolves its element type from the surrounding context: a `return`, a
    // variable reassignment, a field write, and a call argument. None of these carry an inline
    // `int[]` annotation on the literal itself, so each exercises the expected-type threading.
    let code = "
        fun sink(xs: int[]): int { return 0; }
        class Bag { public items: int[]; constructor() { this.items = []; } }
        fun make(): int[] { return []; }
        fun driver(): int {
            let ys: int[] = [1];
            ys = [];
            return sink([]);
        }
    ";
    let (wat, _count) = emit_hir_to_wat(code);
    assert!(
        wat.contains("(func $make"),
        "return-context empty array should emit:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $driver"),
        "assignment/arg empty array should emit:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $Bag_constructor"),
        "field-init empty array should emit:\n{}",
        wat
    );
}

#[test]
fn test_nested_empty_array_infers_element_type() {
    // The expected element type is threaded into each element, so the inner `[]` in `int[][] = [[]]`
    // infers `int[]` (rather than being treated as an untyped `int[][]` and mistyping the outer).
    let code = "
        fun driver(): int {
            let g: int[][] = [[]];
            return 0;
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        !diagnostics.has_errors(),
        "nested empty array should type-check: {:?}",
        diagnostics.diagnostics
    );
    let (wat, _count) = emit_hir_to_wat(code);
    assert!(
        wat.contains("(func $driver"),
        "nested empty array should emit:\n{}",
        wat
    );
}

#[test]
fn test_ambiguous_empty_array_reports_clear_error() {
    // Without any array-typed context there is nothing to infer the element type from, so the
    // literal is rejected with an actionable message (and a real span), not silently dropped.
    let code = "
        fun driver(): int {
            let bad = [];
            return 0;
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        diagnostics.errors().any(|d| d
            .message
            .contains("infer the element type of an empty array")),
        "expected an actionable empty-array error, got: {:?}",
        diagnostics.diagnostics
    );
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
    assert!(
        wat.contains("(call $addup"),
        "call should resolve to the callee symbol:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $Point_getx"),
        "extend method body should emit:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $Point_getx"),
        "call should resolve to the extend method:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $Box_int_peek"),
        "generic extend method should emit:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $Box_int_peek"),
        "call should resolve to the instance:\n{}",
        wat
    );
    assert!(
        !wat.contains("$Box_int_peek__"),
        "no instance suffix on a struct-generic extend:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $Res_del"),
        "destructor body should emit:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $release_Node"),
        "per-type release missing:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $Node_del)"),
        "destructor not invoked from release:\n{}",
        wat
    );
    // The reference field `next` is deep-released; the scalar `v` is not.
    assert!(
        wat.contains("(call $release_Node)"),
        "reference field not released:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $release_object"),
        "tag-dispatch router missing:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $free)"),
        "release must free the block:\n{}",
        wat
    );
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
    assert_eq!(
        count, 2,
        "both the constructor body and make should be emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $Point_constructor"),
        "constructor body should emit:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $malloc)"),
        "construction should allocate:\n{}",
        wat
    );
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
    assert_eq!(
        count, 3,
        "make, read, and the constructor body should be emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $malloc)"),
        "generic construction should allocate:\n{}",
        wat
    );
    assert!(
        wat.contains("(i32.store)"),
        "the field should be initialized:\n{}",
        wat
    );
    assert!(
        wat.contains("(i32.load)"),
        "the field read should lower to a load:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $__dream_init"),
        "missing init function:\n{}",
        wat
    );
    assert!(
        wat.contains("(start $__dream_init)"),
        "init must run at start:\n{}",
        wat
    );
    assert!(
        wat.contains("(global.set $g0)"),
        "init should store the global:\n{}",
        wat
    );
    wat::parse_str(&wat).expect("module with a start-based initializer should assemble");
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
    assert!(
        wat.contains("(call $log)"),
        "call should resolve to the import:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(call $print_int)"),
        "print(int) should call $print_int:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(call $print_string)"),
        "print(string) should call $print_string:\n{}",
        wat
    );
    assert!(
        wat.contains("(data "),
        "the string literal should be interned:\n{}",
        wat
    );
    wat::parse_str(&wat).expect("module printing a string should assemble");
}

#[test]
fn test_hir_emission_print_char() {
    let code = format!("{SYSTEM_STUB} fun run(): void {{ System.print('x'); }}");
    let wat = emit_hir_to_module(&code);
    assert!(
        wat.contains("(call $print_char)"),
        "print(char) should call $print_char:\n{}",
        wat
    );
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
    for helper in [
        "$bool_to_string",
        "$float_to_string",
        "$double_to_string",
        "$long_to_string",
    ] {
        assert!(
            wat.contains(&format!("(call {helper})")),
            "missing {helper} in print:\n{}",
            wat
        );
    }
    assert!(
        wat.contains("(func $bool_to_string"),
        "bool formatter should be defined:\n{}",
        wat
    );
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
    assert!(
        module.contains("(func $run"),
        "an object print should be covered now:\n{}",
        module
    );
    assert!(
        module.contains("(call $print_object)"),
        "object print routes to $print_object:\n{}",
        module
    );
    assert!(
        module.contains("(func $Box_to_string"),
        "a default struct to_string is generated:\n{}",
        module
    );
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
fn exec_int_to_string_via_concat_and_interpolation() {
    // A non-string operand of `+` (and any interpolation hole) is implicitly rendered through the
    // object protocol's `to_string`, so `int` values compose into strings with no explicit call.
    let code = format!(
        "{SYSTEM_STUB}
        fun main(): void {{
            let n: int = 42;
            System.println(\"count = \" + n);
            System.println($\"n is {{n}} and n+1 is {{n + 1}}\");
        }}"
    );
    assert_eq!(
        run_and_capture(&code, "main"),
        "count = 42\nn is 42 and n+1 is 43\n"
    );
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
    assert_eq!(
        run_and_capture(&code, "main"),
        "[Point { x: 1, y: 2 }, Point { x: 3, y: 4 }]\n"
    );
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
/// `fun main() { let f = add; print(f(2, 3)); }`. The analyzer now emits function values itself (see
/// `test_hir_emission_first_class_function`); this hand-built MIR still exercises the backend
/// (FuncRef -> table index, function table + signature, `call_indirect`) in isolation. Returns the
/// interner alongside so its `TypeId`s stay valid.
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
        Rvalue::Binary(
            BinOp::Add,
            Operand::Copy(Place::Local(a)),
            Operand::Copy(Place::Local(b)),
        ),
    );
    ab.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));

    let mut mb = FunctionBuilder::new("main", void);
    mb.set_def(DefId(11), vec![]);
    let f = mb.new_local(functy, Some("f".into()));
    let r = mb.new_local(int, Some("r".into()));
    mb.assign(
        Place::Local(f),
        Rvalue::FuncRef(Callee {
            def: add_def,
            args: vec![],
            ret: int,
        }),
    );
    mb.assign(
        Place::Local(r),
        Rvalue::IndirectCall {
            target: Operand::Copy(Place::Local(f)),
            args: vec![Operand::Const(Const::Int(2)), Operand::Const(Const::Int(3))],
        },
    );
    mb.push(Statement::Print {
        arg: Operand::Copy(Place::Local(r)),
        ty: int,
        newline: false,
    });
    mb.terminate(Terminator::Return(None));

    (
        Mir {
            functions: vec![ab.finish(), mb.finish()],
            ..Default::default()
        },
        i,
    )
}

#[test]
fn test_indirect_call_emits_table_and_signature() {
    let (mir, interner) = indirect_call_demo();
    let wat = crate::mir::emit::emit_module(&mir, &interner, false);
    assert!(
        wat.contains("(table $__ft 2 funcref)"),
        "function table missing:\n{}",
        wat
    );
    assert!(
        wat.contains("(elem (i32.const 0) $add $main)"),
        "elem section missing:\n{}",
        wat
    );
    assert!(
        wat.contains("(type $sig_i32_i32__i32"),
        "call_indirect signature missing:\n{}",
        wat
    );
    assert!(
        wat.contains("(call_indirect $__ft (type $sig_i32_i32__i32))"),
        "indirect call missing:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(call_indirect $__ft"),
        "indirect call not emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("funcref"),
        "function value not emitted:\n{}",
        wat
    );
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
fn dropped_function_reports_error_instead_of_silent_omission() {
    // Printing a function value is not representable in HIR, so `main` is dropped from backend
    // coverage. Historically this compiled "successfully" while emitting no code for `main`; the
    // analyzer must now surface an explicit error rather than silently omitting the function.
    let code = format!(
        "{SYSTEM_STUB}
        fun add(a: int, b: int): int {{ return a + b; }}
        fun main(): void {{ let f: fun(int, int): int = add; System.println(f); }}"
    );
    let diagnostics = analyze_code(&code);
    assert!(
        diagnostics.errors().any(|d| d
            .message
            .contains("not yet supported by the compiler backend")
            && d.message.contains("main")),
        "expected an explicit unsupported-construct error for the dropped function, got: {:?}",
        diagnostics.diagnostics
    );
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
    assert_eq!(
        count, 3,
        "two id instances + driver should be emitted:\n{}",
        wat
    );
    let instances = wat.matches("(func $id__").count();
    assert_eq!(
        instances, 2,
        "each monomorphization gets its own symbol:\n{}",
        wat
    );
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
    // first (`true`/`false`/`-` then the object-protocol `null`/`<object>`/`[`/`]`/`, `/`length`),
    // so the user's `"hi"` follows them at 1228 (each block carries a 4-byte length prefix, no NUL).
    let code = "fun greet(): string { return \"hi\"; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(
        count, 1,
        "the string-returning function should be emitted as HIR"
    );
    assert!(
        wat.contains("(func $greet"),
        "missing emitted function:\n{}",
        wat
    );
    assert!(
        wat.contains("(i32.const 1228)"),
        "string literal should resolve to its data pointer:\n{}",
        wat
    );
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
    assert_eq!(
        count, 3,
        "the field-read, constructor, and constructor-body functions should be emitted"
    );
    assert!(
        wat.contains("(func $getx"),
        "missing field-read function:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $make"),
        "missing constructor function:\n{}",
        wat
    );
    // `p.x` (field 0) lowers to a real load now that the layout is threaded through.
    assert!(
        wat.contains("(i32.load)"),
        "field read should lower to a load:\n{}",
        wat
    );
    // `Point(1, 2)` allocates and initializes fields.
    assert!(
        wat.contains("(call $malloc)"),
        "constructor should allocate:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $bump"),
        "missing field-assignment function:\n{}",
        wat
    );
    // `c.n = ...` lowers to a real store through the field address.
    assert!(
        wat.contains("(i32.store)"),
        "field write should lower to a store:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_index_assignment() {
    // Indexed assignment lowers to an `Assign` with an `Index` place.
    let code = "fun setfirst(xs: int[], v: int): void { xs[0] = v; }";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(count, 1, "the index-assignment function should be emitted");
    assert!(
        wat.contains("(func $setfirst"),
        "missing index-assignment function:\n{}",
        wat
    );
    // `xs[0] = v` computes the element address (base + 4 + i*stride) and stores.
    assert!(
        wat.contains("(i32.store)"),
        "index write should lower to a store:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $pick"),
        "missing enum function:\n{}",
        wat
    );
    // `Color.Green` is the second member, value 1.
    assert!(
        wat.contains("i32.const 1"),
        "missing enum constant:\n{}",
        wat
    );
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
    assert_eq!(
        count, 2,
        "both the method body and its caller should be emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $Box_get"),
        "missing emitted method body:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $use_box"),
        "missing instance-call function:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $Box_get"),
        "instance call should dispatch to the method:\n{}",
        wat
    );
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
    assert_eq!(
        count, 2,
        "both the static method and its caller should be emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $M_id"),
        "missing emitted static method:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $M_id"),
        "static call should dispatch to the method:\n{}",
        wat
    );
}

#[test]
fn test_hir_emission_global_read_and_write() {
    // A module-global resolves to a `Global` binding for both reads and assignments.
    let code = "
        let counter: int = 0;
        fun tick(): int { counter = counter + 1; return counter; }
    ";
    let (wat, count) = emit_hir_to_wat(code);
    assert_eq!(
        count, 1,
        "the global-using function should be emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("global.get $g0"),
        "missing global read:\n{}",
        wat
    );
    assert!(
        wat.contains("global.set $g0"),
        "missing global write:\n{}",
        wat
    );
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
    assert_eq!(
        count, 2,
        "both union constructors should be emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $mk"),
        "missing data-variant constructor:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $nil"),
        "missing unit-variant constructor:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $classify"),
        "missing switch function:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $describe"),
        "missing switch function:\n{}",
        wat
    );
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
    assert!(
        wat.contains("(func $count"),
        "missing array-len function:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $slen"),
        "missing string-len function:\n{}",
        wat
    );
    assert!(
        wat.contains("(call $strlen)"),
        "string len should use $strlen:\n{}",
        wat
    );
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
    assert_eq!(
        count, 1,
        "the switch-expression function should be emitted:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $area"),
        "missing switch-expression function:\n{}",
        wat
    );
}

#[test]
fn test_switch_nested_patterns_are_exhaustive() {
    // Nested union patterns are counted recursively: `Wrap(A(n))` + `Wrap(B)` together cover the
    // `Wrap` variant (all of `Inner`), so with `Bare` the switch is exhaustive without a `_` arm.
    let code = "
        enum Inner { A(v: int), B }
        enum Outer { Wrap(inner: Inner), Bare }
        fun describe(o: Outer): int {
            return switch (o) {
                Wrap(A(n)) => n,
                Wrap(B)    => -1,
                Bare       => 0,
            };
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        !diagnostics.has_errors(),
        "nested patterns should be exhaustive: {:?}",
        diagnostics.diagnostics
    );
}

#[test]
fn test_switch_nested_patterns_incomplete_is_rejected() {
    // Missing an inner variant (`Wrap(C)`) leaves `Wrap` only partially covered, so the switch is
    // still non-exhaustive and must be reported.
    let code = "
        enum Inner { A(v: int), B, C }
        enum Outer { Wrap(inner: Inner), Bare }
        fun describe(o: Outer): int {
            return switch (o) {
                Wrap(A(n)) => n,
                Wrap(B)    => -1,
                Bare       => 0,
            };
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|d| d.message.contains("Non-exhaustive switch")),
        "partial nested coverage should be non-exhaustive: {:?}",
        diagnostics.diagnostics
    );
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
    assert!(
        wat.contains("(func $work"),
        "missing async function:\n{}",
        wat
    );
}

#[test]
fn test_async_emits_scheduler_runtime_and_poll() {
    let code = format!(
        "{ASYNC_STUB}
        async fun delay(): void {{ await Time.sleep(0); }}
        async fun main(): void {{ await delay(); }}"
    );
    let wat = emit_hir_to_module(&code);
    assert!(
        wat.contains("(func $dream_run_loop"),
        "scheduler missing:\n{}",
        wat
    );
    assert!(
        wat.contains("(func $poll_delay"),
        "poll fn missing:\n{}",
        wat
    );
    assert!(
        wat.contains("call $dream_new_future"),
        "constructor missing:\n{}",
        wat
    );
    assert!(
        wat.contains("call $dream_await"),
        "suspend missing:\n{}",
        wat
    );
    assert!(
        wat.contains("(export \"main\")"),
        "async main wrapper missing:\n{}",
        wat
    );
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
