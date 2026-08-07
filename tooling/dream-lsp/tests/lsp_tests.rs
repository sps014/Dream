mod common;

use common::TestHarness;

#[test]
fn hover_on_function_shows_signature() {
    // We place the cursor | at the start of `add` in the call.
    let src = "
fun add(a: int, b: int): int {
    return a + b;
}
fun main(): void {
    let x: int = |add(1, 2);
    println(x);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let hover = index.hover(&harness.src, harness.offset).expect("Expected hover info");
    assert!(hover.contents.contains("fun add"));
}

#[test]
fn definition_resolves_call_to_declaration() {
    let src2 = "
fun add(a: int, b: int): int {
    return a + b;
}
fun main(): void {
    let x: int = a|dd(1, 2);
}
";
    let harness = TestHarness::new(src2);
    let index = harness.index();

    let def = index
        .definition(harness.offset)
        .expect("Expected definition");

    // The definition of `add` should be near the top.
    // The `add` decl starts at `\nfun add` -> offset is roughly around 5.
    let decl_offset = src2.replace("|", "").find("fun add").unwrap();
    assert!(def.0 >= decl_offset);
}

#[test]
fn diagnostics_flags_unknown_variable() {
    let src = "
fun main(): void {
    let y: int = |nope + 1;
}
";
    let harness = TestHarness::new(src);
    let diagnostics = harness.diagnostics();

    let has_error = diagnostics
        .iter()
        .any(|d| d.severity == "error" && d.message.contains("nope"));
    assert!(has_error, "Expected diagnostic for unknown variable 'nope'");
}

#[test]
fn formatting_reindents_by_brace_depth() {
    let src = "fun main(): void {\nlet x: int = 1;\nif (x > 0) {\nprintln(x);\n}\n}\n";
    let formatted = dream_lsp::format::format(src);

    assert!(formatted.contains("\n    let x: int = 1;"));
    assert!(formatted.contains("\n        println(x);"));
}

#[test]
fn completions_include_keywords_and_symbols() {
    let src = "
fun add(a: int, b: int): int {
    return a + b;
}
fun main(): void {
    |
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let comps = index.completions(None, &harness.src, harness.offset);

    let has_add = comps.iter().any(|c| c.0 == "add");
    let has_if = comps.iter().any(|c| c.0 == "if");

    assert!(has_add, "Expected 'add' in completions");
    assert!(has_if, "Expected 'if' in completions");
}

#[test]
fn member_completions_after_dot() {
    let src = "
class Point {
    x: int;
    fun mag(): int { return this.x; }
}
fun main(): void {
    let origin: Point = Point(0);
    origin.|
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let comps = index.completions(None, &harness.src, harness.offset);

    let has_x = comps.iter().any(|c| c.0 == "x");
    let has_enum = comps.iter().any(|c| c.1 == dream_lsp::index::SymKind::Enum);

    assert!(has_x, "Expected 'x' in completions");
    assert!(!has_enum, "Enums should not appear after `.`");
}

#[test]
fn option_local_member_completions() {
    let harness = TestHarness::new(
        "fun main(): void {\n    let o: Option<int> = Option.None;\n    o.|\n}\n",
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    let names: Vec<&str> = comps.iter().map(|(n, ..)| n.as_str()).collect();
    assert!(
        names.contains(&"is_some") && names.contains(&"map") && names.contains(&"unwrap_or"),
        "expected Option instance methods on o., got {names:?}"
    );
    assert!(
        !names.contains(&"Some") && !names.contains(&"None"),
        "variants belong on Option., not on an Option local"
    );
}

#[test]
fn result_inferred_from_gpu_try_init_member_completions() {
    let harness = TestHarness::new(
        "import system.gpu;\nasync fun main(): void {\n    let a = await Gpu.try_init();\n    a.|\n}\n",
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    let names: Vec<&str> = comps.iter().map(|(n, ..)| n.as_str()).collect();
    assert!(
        names.contains(&"is_ok") && names.contains(&"unwrap_or") && names.contains(&"and_then"),
        "expected Result instance methods on a. after await Gpu.try_init, got {names:?}"
    );
}

#[test]
fn async_call_without_await_is_future_not_result() {
    // `Gpu.try_init()` is async → `Future<Result<bool, GpuError>>`. Without `await`, Result
    // methods must not appear (matches the analyzer: Future has no `is_err`).
    let harness = TestHarness::new(
        "import system.gpu;\nfun main(): void {\n    let a = Gpu.try_init();\n    a.|\n}\n",
    );
    let index = harness.index();
    let a_ty = index
        .decls
        .iter()
        .find(|d| d.name == "a" && d.is_main)
        .and_then(|d| d.ty.as_deref());
    assert!(
        a_ty.is_some_and(|t| t.starts_with("Future<") && t.contains("Result")),
        "expected Future<Result<…>> for bare Gpu.try_init(), got {a_ty:?}"
    );
    let comps = index.completions(None, &harness.src, harness.offset);
    let names: Vec<&str> = comps.iter().map(|(n, ..)| n.as_str()).collect();
    assert!(
        !names.contains(&"is_err") && !names.contains(&"is_ok") && !names.contains(&"unwrap_or"),
        "Result methods must not complete on an un-awaited Future: {names:?}"
    );
}

#[test]
fn same_named_methods_resolve_by_receiver() {
    let harness = TestHarness::new(
        r#"
class Alpha {
    public fun run(x: int): int { return x; }
}
class Beta {
    public fun run(s: string): string { return s; }
}
fun main(): void {
    let a = Alpha();
    a.ru|n(1);
}
"#,
    );
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("hover on Alpha.run");
    assert!(
        hover.contents.contains("Alpha.run") && hover.contents.contains("x: int"),
        "expected Alpha.run, got {}",
        hover.contents
    );
    assert!(
        !hover.contents.contains("Beta.run"),
        "must not show Beta.run for Alpha receiver"
    );

    let harness_b = TestHarness::new(
        r#"
class Alpha {
    public fun run(x: int): int { return x; }
}
class Beta {
    public fun run(s: string): string { return s; }
}
fun main(): void {
    Beta.run(|);
}
"#,
    );
    let sig = harness_b
        .index()
        .signature_help(&harness_b.src, harness_b.offset)
        .expect("signature help on Beta.run");
    assert!(
        sig.detail.contains("Beta.run") && sig.detail.contains("s: string"),
        "expected Beta.run signature, got {}",
        sig.detail
    );
    assert!(
        !sig.detail.contains("Alpha.run"),
        "must not show Alpha.run for Beta receiver"
    );
}

#[test]
fn compute_pass_dispatch_not_webworker_pool() {
    let harness = TestHarness::new(
        r#"
import system.gpu;
async fun main(): void {
    let p = ComputePass.begin();
    p.dispa|tch("k", Buffer.alloc<GpuBuffer<float>>(0), 1, 1, 1);
}
"#,
    );
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("hover on ComputePass.dispatch");
    assert!(
        hover.contents.contains("ComputePass.dispatch"),
        "expected ComputePass.dispatch, got {}",
        hover.contents
    );
    assert!(
        !hover.contents.contains("WebWorkerPool"),
        "must not show WebWorkerPool.dispatch: {}",
        hover.contents
    );
    assert!(
        hover.contents.contains("kernel: string"),
        "expected ComputePass param names: {}",
        hover.contents
    );

    let sig_harness = TestHarness::new(
        r#"
import system.gpu;
async fun main(): void {
    let p = ComputePass.begin();
    p.dispatch(|);
}
"#,
    );
    let sig = sig_harness
        .index()
        .signature_help(&sig_harness.src, sig_harness.offset)
        .expect("signature help");
    assert!(
        sig.detail.contains("ComputePass.dispatch") && !sig.detail.contains("WebWorkerPool"),
        "signature help must be ComputePass.dispatch, got {}",
        sig.detail
    );
}

#[test]
fn method_generic_args_expanded_on_hover() {
    let harness = TestHarness::new(
        r#"
import system;
async fun main(): void {
    let pool = WebWorkerPool(2);
    pool.dispa|tch<int, string>(1, (n: int) => "x");
}
"#,
    );
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("hover on dispatch");
    assert!(
        hover.contents.contains("WebWorkerPool.dispatch")
            && hover.contents.contains("msg: int")
            && hover.contents.contains("fun(int): string")
            && hover.contents.contains(": string"),
        "expected TIn/TOut substituted, got {}",
        hover.contents
    );
    assert!(
        !hover.contents.contains("TIn") && !hover.contents.contains("TOut"),
        "type params should be expanded away: {}",
        hover.contents
    );
}

#[test]
fn colliding_fields_resolve_by_receiver() {
    let harness = TestHarness::new(
        r#"
class A { public value: int; }
class B { public value: string; }
fun main(): void {
    let a = A(1);
    let _ = a.val|ue;
}
"#,
    );
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("hover on A.value");
    assert!(
        hover.contents.contains("A.value") && hover.contents.contains("int"),
        "expected A.value: int, got {}",
        hover.contents
    );
    assert!(!hover.contents.contains("B.value") && !hover.contents.contains("string"));
}

#[test]
fn type_name_completion_only_static_methods() {
    let harness = TestHarness::new(
        r#"
import system.gpu;
fun main(): void {
    ComputePass.|
}
"#,
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    let names: Vec<&str> = comps.iter().map(|(n, ..)| n.as_str()).collect();
    assert!(
        names.contains(&"begin"),
        "static begin should appear on ComputePass.: {names:?}"
    );
    assert!(
        !names.contains(&"dispatch") && !names.contains(&"submit"),
        "instance methods must not complete on ComputePass. type name: {names:?}"
    );
}

#[test]
fn instance_completion_excludes_static_methods() {
    let harness = TestHarness::new(
        r#"
import system.gpu;
fun main(): void {
    let p = ComputePass.begin();
    p.|
}
"#,
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    let names: Vec<&str> = comps.iter().map(|(n, ..)| n.as_str()).collect();
    assert!(
        names.contains(&"dispatch") && names.contains(&"submit"),
        "instance methods should appear on p.: {names:?}"
    );
    assert!(
        !names.contains(&"begin"),
        "static begin must not complete on instance receiver: {names:?}"
    );
}

#[test]
fn method_detail_display_omits_fun_keyword() {
    let harness = TestHarness::new(
        r#"
import system.gpu;
async fun main(): void {
    Gpu.try_in|it();
}
"#,
    );
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("hover");
    assert!(
        hover.contents.contains("static async Gpu.try_init"),
        "expected clean static async Owner.name form, got {}",
        hover.contents
    );
    assert!(
        !hover.contents.contains("async fun try_init")
            && !hover.contents.contains("Gpu.async fun"),
        "old `Owner.async fun name` form must be gone: {}",
        hover.contents
    );
}

#[test]
fn bare_async_call_inlay_shows_future() {
    use dream_lsp::index::{Index, InlayKind};
    let src = "
async fun delayedDouble(n: int): int {
    return n * 2;
}
async fun main(): void {
    let a = delayedDouble(10);
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Type)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&": Future<int>"),
        "`let a = delayedDouble(10)` should show `: Future<int>`; got {:?}",
        labels
    );
}

#[test]
fn enum_method_doc_comment_in_completion() {
    let harness = TestHarness::new(
        r#"
enum Box {
    Empty,
    // Returns true when Empty.
    public fun is_empty(): bool { return true; }
}
fun main(): void {
    let b: Box = Box.Empty;
    b.|
}
"#,
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    let is_empty = comps
        .iter()
        .find(|(n, ..)| n == "is_empty")
        .expect("expected is_empty method");
    assert!(
        is_empty
            .3
            .as_ref()
            .is_some_and(|d| d.contains("Returns true when Empty")),
        "expected enum method doc in completion documentation, got {:?}",
        is_empty.3
    );
}

#[test]
fn signature_help_on_function() {
    let src = "
fun add(a: int, b: int): int { return a + b; }
fun main(): void {
    add(|);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let sig = index
        .signature_help(&harness.src, harness.offset)
        .expect("Expected signature help");
    assert_eq!(sig.name, "add");
}

#[test]
fn diagnostics_missing_semicolon_position() {
    let src = "
fun main(): void {
    let y: int = 1|
    let x: int = 2;
}
";
    let harness = TestHarness::new(src);
    let diagnostics = harness.diagnostics();

    let has_error = diagnostics
        .iter()
        .any(|d| d.severity == "error" && d.message.contains("Expected ';'"));
    assert!(has_error, "Expected missing semicolon error");
}

#[test]
fn diagnostics_flags_type_mismatch() {
    let src = "
fun main(): void {
    let y: int = |\"hello\";
}
";
    let harness = TestHarness::new(src);
    let diagnostics = harness.diagnostics();

    let has_error = diagnostics
        .iter()
        .any(|d| d.severity == "error" && d.message.contains("cannot convert"));
    assert!(has_error, "Expected diagnostic for type mismatch");
}

#[test]
fn hover_on_struct_field() {
    let src = "
class User {
    age: int;
}
fun main(): void {
    let u: User = User(20);
    let a: int = u.|age;
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let hover = index.hover(&harness.src, harness.offset).expect("Expected hover info");
    assert!(
        hover.contents.contains("int"),
        "Hover should contain field type"
    );
}

#[test]
fn hover_on_union_variant_shows_signature_and_doc() {
    // Cursor on the `Rect` variant in a constructor call.
    let src = "
enum Shape {
    Circle(radius: int),
    // A rectangle with width and height.
    Rect(width: int, height: int),
    Empty,
}
fun main(): void {
    let s: Shape = Shape.|Rect(3, 4);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let hover = index
        .hover(&harness.src, harness.offset)
        .expect("Expected hover info on union variant");
    assert!(
        hover
            .contents
            .contains("Shape.Rect(width: int, height: int)"),
        "Hover should show the variant payload signature, got: {}",
        hover.contents
    );
    assert!(
        hover
            .contents
            .contains("A rectangle with width and height."),
        "Hover should include the variant doc comment, got: {}",
        hover.contents
    );
}

#[test]
fn hover_on_union_variant_in_switch_arm() {
    let src = "
enum Shape {
    Circle(radius: int),
    Rect(width: int, height: int),
}
fun area(s: Shape): int {
    return switch (s) {
        Circle(r) => r,
        R|ect(w, h) => w * h,
    };
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let hover = index
        .hover(&harness.src, harness.offset)
        .expect("Expected hover info on match-arm variant");
    assert!(
        hover
            .contents
            .contains("Shape.Rect(width: int, height: int)"),
        "Match-arm variant hover should show the payload signature, got: {}",
        hover.contents
    );
}

#[test]
fn hover_on_generic_enum_shows_type_parameters() {
    let src = "
enum Opt<T> {
    Some(value: T),
    None,
}
fun main(): void {
    let o: O|pt<int> = Opt.None;
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let hover = index
        .hover(&harness.src, harness.offset)
        .expect("Expected hover info on generic enum type");
    assert!(
        hover.contents.contains("enum Opt<T>"),
        "Enum hover should include generic parameters, got: {}",
        hover.contents
    );
}

#[test]
fn definition_resolves_union_variant_constructor() {
    let src = "
enum Shape {
    Circle(radius: int),
    Rect(width: int, height: int),
}
fun main(): void {
    let s: Shape = Shape.R|ect(3, 4);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let def = index
        .definition(harness.offset)
        .expect("Expected definition for variant constructor");
    let decl_offset = harness.src.find("Rect").unwrap();
    assert_eq!(def.0, decl_offset, "Should jump to the variant declaration");
}

#[test]
fn signature_help_second_parameter() {
    let src = "
fun add(a: int, b: int): int { return a + b; }
fun main(): void {
    add(1, |);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let sig = index
        .signature_help(&harness.src, harness.offset)
        .expect("Expected signature help");
    assert_eq!(sig.name, "add");
}

#[test]
fn inferred_type_member_completion_forward_reference() {
    let src = "
fun main(): void {
    let u = User(20, \"Alice\");
    u.|
}

class User {
    age: int;
    name: string;
    fun get_age(): int { return this.age; }
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let comps = index.completions(None, &harness.src, harness.offset);
    let has_age = comps.iter().any(|c| c.0 == "age");
    assert!(has_age, "Expected 'age' in completions");
}

#[test]
fn hover_inferred_variable() {
    let src = "
fun main(): void {
    let u = User(20);
    u|
}
class User { age: int; }
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    // Check hover for the reference
    let hover_ref = index
        .hover(&harness.src, harness.offset - 1)
        .expect("Expected hover info on ref");
    assert!(
        hover_ref.contents.contains("User"),
        "Hover should show User type on ref, got {}",
        hover_ref.contents
    );
}

#[test]
fn hover_inferred_variable_after_error() {
    let src = "
fun main(): void {
    let x: int = 1 + ; // ERROR HERE
    let u = User(20);
    u|
}
class User { age: int; }
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    // Check hover for the reference
    let hover_ref = index
        .hover(&harness.src, harness.offset - 1)
        .expect("Expected hover info on ref");
    assert!(
        hover_ref.contents.contains("User"),
        "Hover should show User type on ref, got {}",
        hover_ref.contents
    );
}
#[test]
fn explicit_type_hint_cross_file_inference() {
    let dir = std::env::temp_dir().join("dream_lsp_tests2");
    std::fs::create_dir_all(&dir).unwrap();

    let other_file = dir.join("other.dream");
    std::fs::write(
        &other_file,
        "
class RemoteUser {
    id: int;
    fun get_id(): int { return this.id; }
}
fun fetch_user(): RemoteUser {
    return RemoteUser(42);
}
",
    )
    .unwrap();

    let main_src = "
import other;

fun main(): void {
    let u: RemoteUser = fetch_user();
    u.|
}
";
    let main_file = dir.join("main.dream");
    std::fs::write(&main_file, main_src).unwrap();

    let offset = main_src.find('|').unwrap();
    let src = main_src.replace("|", "");

    let index = dream_lsp::index::Index::build(Some(main_file.to_str().unwrap()), &src);

    let comps = index.completions(Some(main_file.to_str().unwrap()), &src, offset);
    println!("Completions: {:?}", comps);

    let has_id = comps.iter().any(|c| c.0 == "id");
    let has_get_id = comps.iter().any(|c| c.0 == "get_id");

    assert!(has_id, "Expected 'id' in completions");
    assert!(has_get_id, "Expected 'get_id' in completions");
}

#[test]
fn explicit_test_class_cross_file_inference() {
    let dir = std::env::temp_dir().join("dream_lsp_tests_3");
    std::fs::create_dir_all(&dir).unwrap();

    let other_file = dir.join("basic_sum.dream");
    std::fs::write(
        &other_file,
        "
public fun add_numbers(a: int, b: int): int {
    return a + b;
}

public class Test {
    public name: string;
    public age: int;

    constructor(name: string, age: int) {
        this.name = name;
        this.age = age;
    }

    public fun print_name() {
        println(this.name);
    }
}
",
    )
    .unwrap();

    let main_src = "
import basic_sum;

public fun main() {
    let result = add_numbers(10,20);
    let t = Test(\"John\", 20);
    t.|
}
";
    let main_file = dir.join("main.dream");
    std::fs::write(&main_file, main_src).unwrap();

    let offset = main_src.find('|').unwrap();
    let src = main_src.replace("|", "");

    let index = dream_lsp::index::Index::build(Some(main_file.to_str().unwrap()), &src);

    // Check variable `t` type
    let decl_t = index.decls.iter().find(|d| d.name == "t").unwrap();
    assert_eq!(decl_t.ty, Some("Test".to_string()));

    // Check variable `result` type
    let decl_res = index.decls.iter().find(|d| d.name == "result").unwrap();
    assert_eq!(decl_res.ty, Some("int".to_string()));

    let comps = index.completions(Some(main_file.to_str().unwrap()), &src, offset);

    let has_name = comps.iter().any(|c| c.0 == "name");
    let has_print_name = comps.iter().any(|c| c.0 == "print_name");

    assert!(has_name, "Expected 'name' in completions");
    assert!(has_print_name, "Expected 'print_name' in completions");
}

#[test]
fn hover_on_builtin_list_push() {
    let src = "
import system.collections;
fun main(): void {
    let xs = List<int>();
    xs.pu|sh(1);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let hover = index
        .hover(&harness.src, harness.offset)
        .expect("Expected hover info on builtin method");
    println!("HOVER CONTENTS: {}", hover.contents);
    // With generic substitution, it should show 'push(value: int)' instead of 'push(value: T)'
    assert!(hover.contents.contains("push(value: int)"));
    assert!(hover.contents.contains("Appends a value to the end"));
}

#[test]
fn test_hover_math_floor() {
    let src = "
fun main(): void {
    let m = Math.f|loor(3.7);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();
    let hover = index
        .hover(&harness.src, harness.offset)
        .expect("Expected hover info on Math.floor");
    println!("HOVER CONTENTS MATH.FLOOR: {}", hover.contents);
}

#[test]
fn parameter_inlay_hints_on_function_and_constructor_calls() {
    use dream_lsp::index::{Index, InlayKind};
    let src = "
class Point {
    x: int;
    y: int;
    constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }
}
fun add(a: int, b: int): int {
    return a + b;
}
fun main(): void {
    let p = Point(3, 4);
    let s = add(1, 2);
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Parameter)
        .map(|h| h.label.as_str())
        .collect();
    // A custom `constructor` supplies the positional parameter hints.
    assert!(
        labels.contains(&"x:"),
        "expected `x:` hint, got {:?}",
        labels
    );
    assert!(
        labels.contains(&"y:"),
        "expected `y:` hint, got {:?}",
        labels
    );
    // Free function parameters.
    assert!(
        labels.contains(&"a:"),
        "expected `a:` hint, got {:?}",
        labels
    );
    assert!(
        labels.contains(&"b:"),
        "expected `b:` hint, got {:?}",
        labels
    );
}

#[test]
fn parameter_inlay_hints_suppressed_when_arg_matches_name() {
    use dream_lsp::index::{Index, InlayKind};
    let src = "
fun add(a: int, b: int): int {
    return a + b;
}
fun main(): void {
    let a = 1;
    let b = 2;
    let s = add(a, b);
}
";
    let index = Index::build(None, src);
    let param_hints = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Parameter)
        .count();
    assert_eq!(
        param_hints, 0,
        "argument identifiers matching parameter names should not be annotated"
    );
}

#[test]
fn parameter_inlay_hints_on_method_calls() {
    use dream_lsp::index::{Index, InlayKind};
    let src = "
import system.collections;
fun main(): void {
    let nums = List<int>();
    nums.push(42);
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Parameter)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&"value:"),
        "expected `value:` hint on List.push, got {:?}",
        labels
    );
}

#[test]
fn parameter_inlay_hint_anchors_to_start_of_argument() {
    use dream_lsp::index::{Index, InlayKind};
    // A compound argument (`b.v`, `a * b`) must place the parameter-name hint *before* the whole
    // expression, not in the middle of it (regression: it used to land at `.v` / the operator,
    // rendering as `b. p: v`).
    let src = "
class Box {
    v: int;
}
fun take(p: int): void { }
fun main(): void {
    let b = Box(5);
    take(b.v);
}
";
    let index = Index::build(None, src);
    let hint = index
        .inlay_hints
        .iter()
        .find(|h| h.kind == InlayKind::Parameter && h.label == "p:")
        .expect("expected a `p:` parameter hint");
    let after = &src[hint.offset..];
    assert!(
        after.starts_with("b.v"),
        "hint should be anchored at the start of `b.v`, but source after offset is {:?}",
        &after[..after.len().min(8)]
    );
}

#[test]
fn generic_type_inlay_hint_uses_angle_brackets() {
    use dream_lsp::index::{Index, InlayKind};
    let src = "
class Box<T> {
    public value: T;
}
fun main(): void {
    let b = Box<int>(5);
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Type)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&": Box<int>"),
        "generic type hint should read `Box<int>`, not the mangled `Box_int`; got {:?}",
        labels
    );
    assert!(
        !labels.iter().any(|l| l.contains("Box_int")),
        "no inlay hint should expose the mangled `Box_int` form; got {:?}",
        labels
    );
}

#[test]
fn await_call_infers_unwrapped_type() {
    use dream_lsp::index::{Index, InlayKind};
    let src = "
async fun delayedDouble(n: int): int {
    await sleep(100);
    return n * 2;
}
async fun main(): void {
    let a = await delayedDouble(10);
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Type)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&": int"),
        "`let a = await delayedDouble(10)` should show `: int`, not unknown; got {:?}",
        labels
    );
}

#[test]
fn await_in_branch_infers_unwrapped_type() {
    // `await` inside a branch/loop body is a supported suspend point, and the LSP walks into those
    // bodies, so `let a = await g(...)` there still infers the unwrapped awaited type.
    use dream_lsp::index::{Index, InlayKind};
    let src = "
async fun g(n: int): int { return n; }
async fun main(): void {
    let ready = true;
    if (ready) {
        let a = await g(10);
    }
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Type)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&": int"),
        "`let a = await g(10)` inside a branch should show `: int`; got {:?}",
        labels
    );
}

#[test]
fn arithmetic_binary_infers_operand_type() {
    use dream_lsp::index::{Index, InlayKind};
    let src = "
fun main(): void {
    let c: int = 3;
    let a = c * 5;
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Type)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&": int"),
        "`let a = c * 5` should infer `: int`, not unknown; got {:?}",
        labels
    );
}

#[test]
fn hover_on_arithmetic_binding_shows_type() {
    let src = "
fun main(): void {
    let c: int = 3;
    let |a = c * 5;
}
";
    let harness = TestHarness::new(src);
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("Expected hover info on `a`");
    assert!(
        hover.contents.contains("int"),
        "hover on `a` should show `int`, got {:?}",
        hover.contents
    );
}

#[test]
fn arithmetic_binary_falls_back_to_left_operand_double() {
    use dream_lsp::index::{Index, InlayKind};
    // The left operand's type wins, matching the compiler's arithmetic result rule.
    let src = "
fun main(): void {
    let d: double = 1.5;
    let a = d + 1;
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Type)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&": double"),
        "`let a = d + 1` should infer `: double` from the left operand; got {:?}",
        labels
    );
}

#[test]
fn function_value_binding_infers_function_type() {
    use dream_lsp::index::{Index, InlayKind};
    // A bare function name used as a value (`let a = fib;`) should infer the function's
    // first-class type `fun(ParamTypes): ReturnType`, not be left unknown.
    let src = "
fun fib(n: int): int {
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}
fun main(): void {
    let a = fib;
}
";
    let index = Index::build(None, src);
    let labels: Vec<&str> = index
        .inlay_hints
        .iter()
        .filter(|h| h.kind == InlayKind::Type)
        .map(|h| h.label.as_str())
        .collect();
    assert!(
        labels.contains(&": fun(int): int"),
        "`let a = fib` should infer `: fun(int): int`; got {:?}",
        labels
    );
}

#[test]
fn hover_on_function_value_binding_shows_function_type() {
    let src = "
fun fib(n: int): int {
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}
fun main(): void {
    let |a = fib;
}
";
    let harness = TestHarness::new(src);
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("Expected hover info on `a`");
    assert!(
        hover.contents.contains("fun(int): int"),
        "hover on `a` should show `fun(int): int`, got {:?}",
        hover.contents
    );
}

#[test]
fn hover_shows_doc_comment_above_attribute() {
    let src = "
class System {
    /// Prints a value to standard output.
    @intrinsic(\"print\")
    static extern fun print<T>(value: T): void;
}
fun main(): void {
    System.print(1);
}
";
    let offset = src.find("print(1)").unwrap() + 1; // inside the `print` reference
    let index = dream_lsp::index::Index::build(None, src);
    let hover = index.hover(src, offset).expect("expected hover on System.print");
    assert!(
        hover.contents.contains("Prints a value to standard output"),
        "doc comment above an attribute should still appear in hover; got {}",
        hover.contents
    );
}

#[test]
fn hover_doc_comment_ignores_disconnected_block_above_blank_line() {
    // A file-level header comment block, separated by a blank line from the doc comment that
    // directly precedes a declaration, must not be glued onto that declaration's doc comment.
    let src = "
// File-level header describing this whole module.
// Second line of the header.

// The doc comment for Foo.
enum Foo {
    A,
}
fun main(): void {
    let f = Foo.A;
}
";
    let offset = src.find("enum Foo").unwrap() + 5; // inside `Foo`
    let index = dream_lsp::index::Index::build(None, src);
    let hover = index.hover(src, offset).expect("expected hover on Foo");
    assert!(
        hover.contents.contains("The doc comment for Foo"),
        "hover should include the directly-attached doc comment; got {}",
        hover.contents
    );
    assert!(
        !hover.contents.contains("File-level header"),
        "hover should NOT include the disconnected file-header comment; got {}",
        hover.contents
    );
}

#[test]
fn hover_on_global_shows_declaration() {
    let src = "
const FACTOR: int = 10;
fun main(): void {
    let y: int = FA|CTOR + 1;
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();
    let hover = index
        .hover(&harness.src, harness.offset)
        .expect("expected hover on a top-level global");
    assert!(
        hover.contents.contains("FACTOR") && hover.contents.contains("const"),
        "global hover should show its `const` declaration; got {}",
        hover.contents
    );
}

#[test]
fn definition_resolves_global_reference() {
    let src = "
let count: int = 0;
fun main(): void {
    let y: int = co|unt + 1;
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();
    let def = index
        .definition(harness.offset)
        .expect("expected to resolve a global reference to its declaration");
    let decl_offset = harness.src.find("count").unwrap();
    assert_eq!(
        def.0, decl_offset,
        "definition should point at the global decl"
    );
}

#[test]
fn completions_include_top_level_globals() {
    let src = "
let total: int = 5;
fun main(): void {
    |
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();
    let comps = index.completions(None, &harness.src, harness.offset);
    assert!(
        comps.iter().any(|c| c.0 == "total"),
        "expected the global `total` among completions"
    );
}

#[test]
fn references_finds_declaration_and_all_uses() {
    let src = "
fun add(a: int, b: int): int {
    return a + b;
}
fun main(): void {
    let x: int = a|dd(1, 2);
    let y: int = add(3, 4);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();
    let refs = index.references(harness.offset, true);
    // The declaration plus the two call sites.
    assert!(
        refs.len() >= 3,
        "expected at least 3 occurrences of `add`, got {:?}",
        refs
    );
}

#[test]
fn document_symbols_list_top_level_declarations() {
    let src = "
let g: int = 1;
fun foo(): void {}
class Point {
    public x: int;
}
|";
    let harness = TestHarness::new(src);
    let index = harness.index();
    let names: Vec<&str> = index
        .document_symbols()
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    for expected in ["g", "foo", "Point"] {
        assert!(
            names.contains(&expected),
            "expected `{}` in document symbols, got {:?}",
            expected,
            names
        );
    }
}

#[test]
fn workspace_symbols_match_by_substring() {
    let src = "
fun compute(): int { return 1; }
class Container {
    public value: int;
    public fun getValue(): int { return value; }
}
fun main(): void {
    let temp: int = 3;
}
|";
    let harness = TestHarness::new(src);
    let index = harness.index();

    // A substring query matches names case-insensitively across the whole document.
    let names: Vec<&str> = index
        .symbols_matching("value")
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert!(
        names.contains(&"value"),
        "expected field `value` in workspace symbols, got {:?}",
        names
    );
    assert!(
        names.contains(&"getValue"),
        "expected method `getValue` in workspace symbols, got {:?}",
        names
    );
    assert!(
        !names.contains(&"compute"),
        "did not expect `compute` for query `value`, got {:?}",
        names
    );
}

#[test]
fn workspace_symbols_include_functions_types_and_locals() {
    let src = "
fun compute(): int { return 1; }
class Container {
    public value: int;
}
fun main(): void {
    let localThing: int = 3;
}
|";
    let harness = TestHarness::new(src);
    let index = harness.index();

    // An empty query returns every named declaration, including function-scoped locals (which
    // document symbols exclude) but never parameters/keywords/type references.
    let names: Vec<&str> = index
        .symbols_matching("")
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    for expected in ["compute", "Container", "value", "main", "localThing"] {
        assert!(
            names.contains(&expected),
            "expected `{}` in workspace symbols, got {:?}",
            expected,
            names
        );
    }
}

#[test]
fn test_incremental_change_re_indexes_document() {
    use dream_lsp::index::Index;

    let src = "
fun main(): void {
    let old_var = 10;
}
";
    // Simulate first edit
    let index1 = Index::build(None, src);
    let has_old = index1.decls.iter().any(|d| d.name == "old_var");
    assert!(has_old, "Expected old_var to be indexed");

    // Apply incremental text edit directly to string, as happens in `backend.rs`
    let mut text = src.to_string();
    let old_len = "old_var".len();
    let offset = text.find("old_var").unwrap();
    text.replace_range(offset..(offset + old_len), "new_var");

    let index2 = Index::build(None, &text);
    let has_old = index2.decls.iter().any(|d| d.name == "old_var");
    let has_new = index2.decls.iter().any(|d| d.name == "new_var");
    assert!(!has_old, "Expected old_var to be removed");
    assert!(has_new, "Expected new_var to be indexed");
}

#[test]
fn auto_import_code_action_for_http_client() {
    use dream_lsp::code_actions::{already_imports, auto_import_actions};
    use tower_lsp::lsp_types::Url;

    let src = r#"
fun main(): void {
    let c = HttpClient("");
}
"#;
    let uri = Url::parse("file:///tmp/main.dream").unwrap();
    let actions = auto_import_actions(&uri, src, "HttpClient");
    assert_eq!(actions.len(), 1);
    // Apply conceptually: insert should mention system.net
    assert!(!already_imports(src, "system.net"));
    let edited = {
        let (_, pos) = dream_lsp::code_actions::import_insert_point(src);
        assert_eq!(pos.line, 0);
        format!("import system.net;\n{}", src)
    };
    assert!(already_imports(&edited, "system.net"));
}

#[test]
fn auto_import_skipped_when_already_imported() {
    use dream_lsp::code_actions::auto_import_actions;
    use tower_lsp::lsp_types::Url;

    let src = "import system.net;\nfun main(): void { let c = HttpClient(\"\"); }\n";
    let uri = Url::parse("file:///tmp/main.dream").unwrap();
    let actions = auto_import_actions(&uri, src, "HttpClient");
    assert!(actions.is_empty());
}

#[test]
fn bootstrap_symbol_has_no_auto_import() {
    use dream_lsp::code_actions::auto_import_actions;
    use tower_lsp::lsp_types::Url;

    let src = "fun main(): void { let o = Option.None; }\n";
    let uri = Url::parse("file:///tmp/main.dream").unwrap();
    let actions = auto_import_actions(&uri, src, "Option");
    assert!(
        actions.is_empty(),
        "Option is bootstrap — no import quick fix"
    );
}

#[test]
fn completion_additional_edits_for_list() {
    use dream_lsp::code_actions::{import_text_edits, unloaded_stdlib_completions};

    let src = "fun main(): void {\n    \n}\n";
    let comps = unloaded_stdlib_completions(src);
    assert!(
        comps
            .iter()
            .any(|(n, pkg, _)| n == "List" && *pkg == "system.collections"),
        "expected List -> system.collections among unloaded completions"
    );
    let edits = import_text_edits(src, "system.collections").expect("edits");
    assert_eq!(edits.len(), 1);
    assert!(edits[0].new_text.contains("import system.collections;"));
}

#[test]
fn import_path_completions_at_import() {
    use dream_lsp::index::SymKind;

    let harness = TestHarness::new("import |\n");
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        comps
            .iter()
            .any(|(n, k, d, _)| n == "system.collections"
                && *k == SymKind::Module
                && d == "stdlib package"),
        "expected system.collections among import completions, got {:?}",
        comps
            .iter()
            .map(|(n, _, d, _)| format!("{n}/{d}"))
            .collect::<Vec<_>>()
    );
    assert!(
        comps.iter().any(|(n, ..)| n == "system"),
        "expected root package `system`"
    );
    assert!(
        !comps
            .iter()
            .any(|(n, ..)| n == "system.core" || n == "system.primitives"),
        "bootstrap packages must not be suggested"
    );
}

#[test]
fn import_path_completions_system_dot() {
    let harness = TestHarness::new("import system.|\n");
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        comps.iter().any(|(n, ..)| n == "system.collections"),
        "expected system.collections after `import system.`"
    );
    assert!(
        comps.iter().all(|(n, ..)| n.starts_with("system.")),
        "after `import system.` every candidate should be under system.*"
    );
}

#[test]
fn import_path_skips_already_imported() {
    let harness = TestHarness::new("import system.collections;\nimport |\n");
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        !comps.iter().any(|(n, ..)| n == "system.collections"),
        "already-imported package should be omitted"
    );
}

#[test]
fn system_dot_in_body_is_class_members_not_packages() {
    // `System.` in an expression is the class, not the `system.*` package tree.
    let harness = TestHarness::new(
        "import system;\nfun main(): void {\n    System.|\n}\n",
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        !comps
            .iter()
            .any(|(n, _, d, _)| d == "stdlib package" || n == "system.collections"),
        "package paths must not appear on System. member completion: {:?}",
        comps
            .iter()
            .map(|(n, _, d, _)| format!("{n}/{d}"))
            .collect::<Vec<_>>()
    );
    assert!(
        comps.iter().any(|(n, ..)| n == "println" || n == "print"),
        "expected System class members, got {:?}",
        comps.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn member_context_skips_unloaded_stdlib_types() {
    use dream_lsp::code_actions::unloaded_stdlib_completions;
    use dream_lsp::index::is_member_completion_context;

    // Only `system` is imported — List/DateTime would normally be offered as unloaded
    // completions, but never after a member-access `.`.
    let harness = TestHarness::new(
        "import system;\nfun main(): void {\n    System.|\n}\n",
    );
    assert!(
        is_member_completion_context(&harness.src, harness.offset),
        "cursor after System. is member context"
    );
    let unloaded = unloaded_stdlib_completions(&harness.src);
    assert!(
        unloaded.iter().any(|(n, ..)| n == "List"),
        "sanity: List is among unloaded stdlib symbols when collections is not imported"
    );
    // Backend merges unloaded only when !is_member_completion_context — index alone
    // already excludes them from member_completions.
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        !comps.iter().any(|(n, ..)| n == "List" || n == "DateTime" || n == "Gpu"),
        "member completion must not include unloaded package types: {:?}",
        comps.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn attribute_name_completions_after_at() {
    use dream_lsp::index::SymKind;

    let harness = TestHarness::new("@|\nclass Foo {}\n");
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        comps
            .iter()
            .any(|(n, k, _, doc)| n == "json"
                && *k == SymKind::Decorator
                && doc.as_ref().is_some_and(|d| d.contains("JSON"))),
        "expected @json with docs, got {:?}",
        comps
            .iter()
            .map(|(n, _, _, d)| format!("{n}/{:?}", d.as_ref().map(|s| &s[..s.len().min(40)])))
            .collect::<Vec<_>>()
    );
    assert!(
        comps.iter().any(|(n, ..)| n == "intrinsic"),
        "expected @intrinsic among attribute names"
    );
}

#[test]
fn attribute_name_completions_partial() {
    let harness = TestHarness::new("@js|\nstatic extern fun f(): void;\n");
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        comps.iter().any(|(n, ..)| n == "json" || n == "json_ignore" || n == "js"),
        "expected js* attributes for @js, got {:?}",
        comps.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>()
    );
    assert!(
        !comps.iter().any(|(n, ..)| n == "intrinsic"),
        "intrinsic must not match partial `js`"
    );
}

#[test]
fn hover_on_attribute_shows_docs() {
    // Cursor on the `json` attribute name.
    let harness = TestHarness::new(
        "@js|on\nclass Foo { public x: int; }\n",
    );
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("expected hover on @json");
    assert!(
        hover.contents.contains("@json") && hover.contents.contains("JSON"),
        "expected attribute docs, got {}",
        hover.contents
    );
}

#[test]
fn hover_on_intrinsic_attribute() {
    let harness = TestHarness::new(
        r#"
class System {
    @intr|insic("print")
    static extern fun print(): void;
}
"#,
    );
    let hover = harness
        .index()
        .hover(&harness.src, harness.offset)
        .expect("expected hover on @intrinsic");
    assert!(
        hover.contents.contains("@intrinsic") && hover.contents.contains("intrinsic"),
        "expected @intrinsic hover docs, got {}",
        hover.contents
    );
}

#[test]
fn signature_help_inside_js_attribute() {
    let harness = TestHarness::new(
        r#"
@js("Dream", |)
static extern fun f(): void;
"#,
    );
    let sig = harness
        .index()
        .signature_help(&harness.src, harness.offset)
        .expect("expected signature help for @js");
    assert!(
        sig.detail.contains("@js") && sig.detail.contains("string"),
        "expected @js signature, got {}",
        sig.detail
    );
    assert!(
        sig.doc_comment
            .as_ref()
            .is_some_and(|d| d.contains("JavaScript")),
        "expected @js doc on signature help"
    );
}

#[test]
fn intrinsic_arg_completions_inside_string() {
    let harness = TestHarness::new(
        r#"
@intrinsic("|")
static extern fun print(): void;
"#,
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        comps.iter().any(|(n, ..)| n == "print" || n == "println"),
        "expected intrinsic keys inside @intrinsic(\"\"), got {:?}",
        comps.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn operator_arg_completions() {
    let harness = TestHarness::new(
        r#"
class Vec {
    @operator("|")
    public fun add(other: Vec): Vec { return other; }
}
"#,
    );
    let comps = harness.index().completions(None, &harness.src, harness.offset);
    assert!(
        comps.iter().any(|(n, ..)| n == "+" || n == "=="),
        "expected operator symbols, got {:?}",
        comps.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn semantic_tokens_distinguish_class_struct_enum_interface() {
    use dream_lsp::semantic_tokens::{compute, TOKEN_TYPES};
    use tower_lsp::lsp_types::SemanticTokenType;

    let src = "\
class MyClass { public x: int; }
struct MyStruct { public y: int; }
enum MyEnum { A, B }
interface MyIface { fun f(): void; }
fun main(): void {
    let a: MyClass = MyClass(1);
    let b: MyStruct = MyStruct(2);
    let c: MyEnum = MyEnum.A;
}
";
    let tokens = compute(None, src);

    // Decode absolute (line, char) positions from LSP delta encoding.
    let mut line = 0u32;
    let mut character = 0u32;
    let mut by_name: Vec<(String, u32)> = Vec::new();
    for t in &tokens {
        if t.delta_line > 0 {
            line += t.delta_line;
            character = t.delta_start;
        } else {
            character += t.delta_start;
        }
        let start = line_char_to_offset(src, line, character);
        let end = start + t.length as usize;
        if end <= src.len() {
            by_name.push((src[start..end].to_string(), t.token_type));
        }
    }

    let class_idx = token_type_index(&TOKEN_TYPES, &SemanticTokenType::CLASS);
    let struct_idx = token_type_index(&TOKEN_TYPES, &SemanticTokenType::STRUCT);
    let enum_idx = token_type_index(&TOKEN_TYPES, &SemanticTokenType::ENUM);
    let iface_idx = token_type_index(&TOKEN_TYPES, &SemanticTokenType::INTERFACE);

    let kind_of = |name: &str| {
        by_name
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, k)| *k)
            .unwrap_or_else(|| panic!("missing semantic token for `{name}`"))
    };

    assert_eq!(kind_of("MyClass"), class_idx, "class decl should be CLASS");
    assert_eq!(
        kind_of("MyStruct"),
        struct_idx,
        "struct decl should be STRUCT"
    );
    assert_eq!(kind_of("MyEnum"), enum_idx, "enum decl should be ENUM");
    assert_eq!(
        kind_of("MyIface"),
        iface_idx,
        "interface decl should be INTERFACE"
    );

    assert_ne!(class_idx, struct_idx);
    assert_ne!(class_idx, enum_idx);
    assert_ne!(struct_idx, enum_idx);

    // Type-position `MyStruct` (in `let b: MyStruct`) must stay STRUCT, not generic `type`.
    let struct_occurrences: Vec<u32> = by_name
        .iter()
        .filter(|(n, _)| *n == "MyStruct")
        .map(|(_, k)| *k)
        .collect();
    assert!(
        struct_occurrences.contains(&struct_idx),
        "expected at least one STRUCT token for MyStruct, got {struct_occurrences:?}"
    );
    // Decl + type annotation (constructor call may be FUNCTION).
    assert!(
        struct_occurrences
            .iter()
            .filter(|&&k| k == struct_idx)
            .count()
            >= 2,
        "expected decl + type-position STRUCT for MyStruct, got {struct_occurrences:?}"
    );
}

fn token_type_index(
    legend: &[tower_lsp::lsp_types::SemanticTokenType],
    want: &tower_lsp::lsp_types::SemanticTokenType,
) -> u32 {
    legend
        .iter()
        .position(|t| t == want)
        .expect("token type missing from legend") as u32
}

fn line_char_to_offset(src: &str, line: u32, character: u32) -> usize {
    let mut cur_line = 0u32;
    let mut offset = 0usize;
    for (i, ch) in src.char_indices() {
        if cur_line == line {
            // `character` is UTF-16 code units in LSP; Dream sources here are ASCII.
            return i + character as usize;
        }
        if ch == '\n' {
            cur_line += 1;
            offset = i + 1;
        }
    }
    if cur_line == line {
        return offset + character as usize;
    }
    src.len()
}
