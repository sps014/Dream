//! Diagnostics tests: clean code is quiet, broken code is flagged, and—crucially for editing—
//! semantic diagnostics keep flowing even while the document has a syntax error (the parser
//! recovers and the analyzer runs over whatever parsed).

mod common;

use common::TestHarness;

#[test]
fn clean_program_has_no_errors() {
    let harness =
        TestHarness::new("fun main(): void {\n    let x: int = 1;\n    let y: int = x + 2;\n}\n|");
    let diagnostics = harness.diagnostics();
    assert!(
        diagnostics.iter().all(|d| d.severity != "error"),
        "expected no errors for a clean program, got: {:?}",
        diagnostics
    );
}

#[test]
fn debug_probes_analyze_cleanly() {
    // The `Debug.*` allocator probes are stdlib intrinsics; calling them (including the
    // `ref_count(object)` probe with a reference argument) must not produce "variable Debug does
    // not exist" or any other error. Guards against the prelude/analyzer drifting.
    let harness = TestHarness::new(
        "fun main(): void {\n    let n = [1, 2];\n    let a: int = Debug.free_list_head();\n    let b: int = Debug.heap_ptr();\n    let c: int = Debug.live_objects();\n    let d: int = Debug.total_allocations();\n    let e: int = Debug.ref_count(n);\n}\n|",
    );
    let diagnostics = harness.diagnostics();
    assert!(
        diagnostics.iter().all(|d| d.severity != "error"),
        "Debug probes should analyze without errors, got: {:?}",
        diagnostics
    );
}

#[test]
fn await_in_control_flow_is_clean() {
    // `await` is allowed anywhere inside an `async` function — inside a branch, a loop body, a
    // ternary arm, and the right operand of `&&`. The LSP shares the compiler analyzer, so none of
    // these should surface a diagnostic (guards against the old "conditionally-evaluated" rejection).
    let src = "async fun g(n: int): int { return n; }\n\
async fun ready(): bool { return true; }\n\
async fun main(): void {\n\
    let flag = true;\n\
    if (flag) { let a = await g(1); }\n\
    let i = 0;\n\
    while (i < 3) { let b = await g(i); i = i + 1; }\n\
    let c = flag ? await g(2) : await g(3);\n\
    let d = flag && await ready();\n\
}\n|";
    let harness = TestHarness::new(src);
    let diagnostics = harness.diagnostics();
    assert!(
        diagnostics.iter().all(|d| d.severity != "error"),
        "await in control-flow positions should be error-free, got: {:?}",
        diagnostics
    );
}

#[test]
fn await_outside_async_is_flagged() {
    // The one remaining placement rule: awaiting in a non-async function is still an error.
    let src = "async fun g(): int { return 1; }\n\
fun main(): void {\n\
    let x = await g();\n\
}\n|";
    let harness = TestHarness::new(src);
    let diagnostics = harness.diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == "error" && d.message.contains("async")),
        "await outside an async function should be flagged, got: {:?}",
        diagnostics
    );
}

#[test]
fn unknown_identifier_is_flagged() {
    let harness = TestHarness::new("fun main(): void {\n    let y: int = nope + 1;\n}\n|");
    let diagnostics = harness.diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == "error" && d.message.contains("nope")),
        "expected an error mentioning the unknown identifier, got: {:?}",
        diagnostics
    );
}

#[test]
fn semantic_diagnostics_survive_a_syntax_error() {
    // The first function has a syntax error; the batch compiler would stop here and report nothing
    // about `main`. The editor instead recovers and still flags the undefined `nope` in `main`.
    let harness = TestHarness::new(
        "fun broken(): void {\n    let a: int = ;\n}\nfun main(): void {\n    let y: int = nope + 1;\n}\n|",
    );
    let diagnostics = harness.diagnostics();
    assert!(
        diagnostics.iter().any(|d| d.message.contains("nope")),
        "expected semantic diagnostics under syntax-error recovery, got: {:?}",
        diagnostics
    );
}
