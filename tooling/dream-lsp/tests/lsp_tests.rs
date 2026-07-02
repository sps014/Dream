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

    let hover = index
        .hover(harness.offset, src)
        .expect("Expected hover info");
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

    let hover = index
        .hover(harness.offset, src)
        .expect("Expected hover info");
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
        .hover(harness.offset, &harness.src)
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
        .hover(harness.offset, &harness.src)
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
        .hover(harness.offset, &harness.src)
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
        .hover(harness.offset - 1, src)
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
        .hover(harness.offset - 1, src)
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
fun main(): void {
    let xs = List<int>();
    xs.pu|sh(1);
}
";
    let harness = TestHarness::new(src);
    let index = harness.index();

    let hover = index
        .hover(harness.offset, src)
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
        .hover(harness.offset, src)
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
        .hover(harness.offset, &harness.src)
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
        .hover(harness.offset, &harness.src)
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
    let hover = index
        .hover(offset, src)
        .expect("expected hover on System.print");
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
    let hover = index.hover(offset, src).expect("expected hover on Foo");
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
        .hover(harness.offset, &harness.src)
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
