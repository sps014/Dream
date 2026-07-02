use super::*;
use crate::lexer::Lexer;
use crate::nodes::{ExpressionNode, StatementNode};
use pretty_assertions::assert_eq;

fn parse_code<'a>(code: &str, arena: &'a bumpalo::Bump) -> (ProgramNode<'a>, DiagnosticBag) {
    let mut diagnostics = DiagnosticBag::new(None);
    let lexer = Lexer::new(code.to_string());
    let mut parser = Parser::new(lexer, arena, &mut diagnostics);
    let tree = parser.parse().unwrap_or_else(|_| {
        crate::syntax_tree::SyntaxTree::new(ProgramNode::new(
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ))
    });
    (tree.get_root().clone(), diagnostics)
}

#[test]
fn test_parse_function_declaration() {
    let code = "fun main(): int { return 42; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.functions.len(), 1);

    let func = &program.functions[0];
    assert_eq!(func.name.text, "main");
    assert!(matches!(func.return_type, Some(Type::Integer(_))));
    assert_eq!(func.parameters.len(), 0);
    assert_eq!(func.body.len(), 1);

    if let StatementNode::Return(Some(ExpressionNode::Literal(Type::Integer(t)))) = &func.body[0] {
        assert_eq!(t.text, "42");
    } else {
        panic!("Expected return statement with integer literal");
    }
}

#[test]
fn test_parse_array_declaration_and_assignment() {
    let code = "fun test(): void { let arr: int[] = [1, 2, 3]; arr[0] = 5; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    assert_eq!(func.body.len(), 2);

    // Check declaration
    if let StatementNode::Declaration(
        id,
        type_annotation,
        ExpressionNode::ArrayLiteral(elements),
        _,
    ) = &func.body[0]
    {
        assert_eq!(id.text, "arr");
        assert!(type_annotation.is_some());
        assert_eq!(elements.len(), 3);
    } else {
        panic!("Expected array declaration");
    }

    // Check index assignment
    if let StatementNode::IndexAssignment(arr_expr, index, value) = &func.body[1] {
        if let ExpressionNode::Identifier(id) = *arr_expr {
            assert_eq!(id.text, "arr");
        } else {
            panic!("Expected identifier in index assignment");
        }
        assert!(matches!(**index, ExpressionNode::Literal(Type::Integer(_))));
        assert!(matches!(value, ExpressionNode::Literal(Type::Integer(_))));
    } else {
        panic!("Expected index assignment");
    }
}

#[test]
fn test_parse_binary_expression_precedence() {
    let code = "fun test(): void { let x = 1 + 2 * 3; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];

    if let StatementNode::Declaration(_, _, ExpressionNode::Binary(left, opr, right), _) =
        &func.body[0]
    {
        assert_eq!(opr.kind, TokenKind::PlusToken);
        assert!(matches!(**left, ExpressionNode::Literal(Type::Integer(_))));
        assert!(matches!(**right, ExpressionNode::Binary(_, _, _))); // The * should be grouped on the right
    } else {
        panic!("Expected binary expression with correct precedence");
    }
}

#[test]
fn test_parse_extern_function() {
    let code = "extern fun alert(msg: string): void;";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.functions.len(), 1);

    let func = &program.functions[0];
    assert_eq!(func.name.text, "alert");
    assert!(func.is_extern);
    assert_eq!(func.body.len(), 0);
    assert_eq!(func.parameters.len(), 1);
    // Defaults: import module "env", import name = function name.
    let js_attr = func.attributes.iter().find(|a| a.name.text == "js");
    assert!(js_attr.is_none());
}

#[test]
fn test_parse_extern_with_js_attribute() {
    let code = "@js(\"dom\", \"setText\") extern fun set_text(v: string): int;";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    assert!(func.is_extern);
    let js_attr = func
        .attributes
        .iter()
        .find(|a| a.name.text == "js")
        .unwrap();
    assert_eq!(js_attr.args.first().unwrap().text, "\"dom\"");
    assert_eq!(js_attr.args.get(1).unwrap().text, "\"setText\"");
}

#[test]
fn test_parse_extern_rejects_body() {
    let code = "extern fun bad(): void { return; }";
    let arena = bumpalo::Bump::new();
    let (_, diagnostics) = parse_code(code, &arena);

    // A body where a `;` is expected must produce a diagnostic.
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_parse_enum_declaration() {
    let code = "enum Color { Red, Green = 5, Blue }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.enums.len(), 1);

    let decl = &program.enums[0];
    assert_eq!(decl.name.text, "Color");
    assert_eq!(decl.variants.len(), 3);
    // Auto-assigned, explicit, then continues from explicit value.
    assert_eq!(decl.variants[0].name.text, "Red");
    assert_eq!(decl.variants[0].value, 0);
    assert_eq!(decl.variants[1].name.text, "Green");
    assert_eq!(decl.variants[1].value, 5);
    assert_eq!(decl.variants[2].name.text, "Blue");
    assert_eq!(decl.variants[2].value, 6);
}

#[test]
fn test_parse_data_enum_with_generics() {
    let code = "enum Option<T> { Some(value: T), None }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.enums.len(), 1);

    let decl = &program.enums[0];
    assert_eq!(decl.name.text, "Option");
    assert!(decl.is_data_enum());
    let params = decl.generic_parameters.as_ref().expect("generic params");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].text, "T");

    assert_eq!(decl.variants.len(), 2);
    assert_eq!(decl.variants[0].name.text, "Some");
    assert_eq!(decl.variants[0].fields.len(), 1);
    assert_eq!(decl.variants[0].fields[0].name.text, "value");
    assert_eq!(decl.variants[1].name.text, "None");
    assert_eq!(decl.variants[1].fields.len(), 0);
}

#[test]
fn test_parse_generic_constraints() {
    // `<T : Iface (+ Iface)*>` on a struct/class and a function records each bound as a
    // `GenericConstraint`; the parameter still appears in `generic_parameters`.
    let code = "\
        struct Sorted<T : Comparable<T> + Equatable<T>> { public v: T; }\n\
        fun max_of<U : Comparable<U>>(a: U, b: U): U { return a; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false, "constraint syntax should parse cleanly");

    let s = &program.structs[0];
    let params = s.generic_parameters.as_ref().expect("generic params");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].text, "T");
    assert_eq!(s.generic_constraints.len(), 1);
    assert_eq!(s.generic_constraints[0].param.text, "T");
    assert_eq!(s.generic_constraints[0].bounds.len(), 2, "T has two interface bounds");

    let f = program.functions.iter().find(|f| f.name.text == "max_of").expect("max_of");
    assert_eq!(f.generic_constraints.len(), 1);
    assert_eq!(f.generic_constraints[0].param.text, "U");
    assert_eq!(f.generic_constraints[0].bounds.len(), 1);
}

#[test]
fn test_parse_switch_expression_with_patterns() {
    let code = "fun f(s: Shape): int { return switch (s) { Circle(r) => r, Empty => 0, _ => 1 }; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    let StatementNode::Return(Some(ExpressionNode::Switch(_subject, arms))) = &func.body[0] else {
        panic!("expected a return of a switch expression");
    };
    assert_eq!(arms.len(), 3);

    use crate::nodes::PatternNode;
    assert!(matches!(arms[0].pattern, PatternNode::Variant(_, _, _)));
    assert!(matches!(arms[2].pattern, PatternNode::Wildcard(_)));
}

#[test]
fn test_parse_switch_arm_guard() {
    let code = "fun f(o: Option): int { return switch (o) { Some(n) if n > 0 => n, _ => 0 }; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    let StatementNode::Return(Some(ExpressionNode::Switch(_subject, arms))) = &func.body[0] else {
        panic!("expected a return of a switch expression");
    };
    assert!(arms[0].guard.is_some(), "first arm should have a guard");
    assert!(arms[1].guard.is_none());
}

#[test]
fn test_parse_switch_statement_pattern_arms() {
    // A pattern-arm `switch` used as a statement parses to an `ExpressionStatement` wrapping an
    // `ExpressionNode::Switch` (distinct from the C-style `case`/`default` form).
    let code = "fun f(o: Option): void { switch (o) { Some(n) => { System.println(n); } None => {} } }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    let StatementNode::ExpressionStatement(ExpressionNode::Switch(_subject, arms)) = &func.body[0]
    else {
        panic!("expected a statement-position switch expression");
    };
    assert_eq!(arms.len(), 2);
}

#[test]
fn test_parse_interpolated_string() {
    // `$"{y+68} is {x}"` desugars to `"" + (y + 68) + " is " + (x)`.
    let code = "fun f(x: int, y: int): string { return $\"{y+68} is {x}\"; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    let StatementNode::Return(Some(ExpressionNode::Binary(left, opr, right))) = &func.body[0]
    else {
        panic!("expected a binary concat chain");
    };
    assert_eq!(opr.kind, TokenKind::PlusToken);
    // Rightmost segment is the `{x}` hole.
    assert!(matches!(&**right, ExpressionNode::Identifier(t) if t.text == "x"));

    // Next on the left spine is the `" is "` literal text segment.
    let ExpressionNode::Binary(l2, _, mid) = &**left else {
        panic!("expected nested binary for ' is ' literal");
    };
    assert!(matches!(&**mid, ExpressionNode::Literal(Type::String(t)) if t.text == "\" is \""));

    // Then the empty-string seed and the `y + 68` hole.
    let ExpressionNode::Binary(seed, _, y_expr) = &**l2 else {
        panic!("expected nested binary for seed + (y + 68)");
    };
    assert!(matches!(&**seed, ExpressionNode::Literal(Type::String(t)) if t.text == "\"\""));
    let ExpressionNode::Binary(y_left, y_opr, y_right) = &**y_expr else {
        panic!("expected the embedded y + 68 binary");
    };
    assert_eq!(y_opr.kind, TokenKind::PlusToken);
    assert!(matches!(&**y_left, ExpressionNode::Identifier(t) if t.text == "y"));
    assert!(matches!(&**y_right, ExpressionNode::Literal(Type::Integer(t)) if t.text == "68"));
}

#[test]
fn test_interpolation_hole_spans_are_absolute() {
    // The identifier `x` inside the hole must carry a file-relative span (not hole-relative) so
    // IDE features (hover, go-to-definition) resolve at the cursor.
    let code = "fun f(x: int): string { return $\"v={x}\"; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    let StatementNode::Return(Some(ExpressionNode::Binary(_, _, right))) = &func.body[0] else {
        panic!("expected a binary concat chain");
    };
    let ExpressionNode::Identifier(tok) = &**right else {
        panic!("expected the `x` hole identifier on the right");
    };
    // `x` in the hole is the second `x` in the source (after the parameter `x`).
    let expected = code.rfind('x').unwrap();
    assert_eq!(tok.text, "x");
    assert_eq!(tok.position.start, expected);
    assert_eq!(tok.position.end, expected + 1);
}

#[test]
fn test_parse_interpolated_string_brace_escapes() {
    // `{{` / `}}` are literal braces and must not open a hole, so this has no embedded expression.
    let code = "fun f(): string { return $\"{{x}}\"; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    let StatementNode::Return(Some(ExpressionNode::Binary(_, opr, right))) = &func.body[0] else {
        panic!("expected a binary concat chain");
    };
    assert_eq!(opr.kind, TokenKind::PlusToken);
    // The whole body collapses to the literal text `{x}` (escapes unwrapped), no hole.
    assert!(matches!(&**right, ExpressionNode::Literal(Type::String(t)) if t.text == "\"{x}\""));
}

#[test]
fn test_match_is_an_ordinary_identifier() {
    // `match` is no longer a keyword, so it is usable as a method name (the stdlib `regex.match`).
    let code = "fun f(r: Regex): string[] { return r.match(\"x\"); }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    let StatementNode::Return(Some(ExpressionNode::MethodCall(_obj, method, _, _))) = &func.body[0]
    else {
        panic!("expected a method call");
    };
    assert_eq!(method.text, "match");
}

#[test]
fn test_parse_do_while() {
    let code = "fun test(): void { do { print_int(1); } while (false); }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    assert!(matches!(func.body[0], StatementNode::DoWhile(_, _)));
}

#[test]
fn test_parse_const_and_labeled_break() {
    let code = "fun test(): void { const x: int = 1; loop: while (true) { break loop; } }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    // First statement is a const declaration (is_const == true).
    assert!(matches!(
        &func.body[0],
        StatementNode::Declaration(_, _, _, true)
    ));
    // Second statement is a labeled loop containing a `break loop;`.
    if let StatementNode::Labeled(label, inner) = &func.body[1] {
        assert_eq!(label, "loop");
        if let StatementNode::While(_, body) = inner {
            assert!(matches!(&body[0], StatementNode::Break(Some(l)) if l == "loop"));
        } else {
            panic!("Expected labeled while loop");
        }
    } else {
        panic!("Expected labeled statement");
    }
}

#[test]
fn test_parse_char_literal() {
    let code = "fun test(): void { let c: char = 'A'; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    if let StatementNode::Declaration(_, _, ExpressionNode::Literal(Type::Char(t)), _) =
        &func.body[0]
    {
        assert_eq!(t.text, "65");
    } else {
        panic!("Expected char literal with code point 65");
    }
}

#[test]
fn test_parse_suffixed_number_literals() {
    // The suffix selects the literal's concrete numeric type and is stripped from the token text.
    let code = "fun test(): void {
        let a = 42L;
        let b = 7u;
        let c = 9uL;
        let d = 255b;
    }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);
    assert_eq!(diagnostics.has_errors(), false);
    let body = &program.functions[0].body;

    assert!(
        matches!(&body[0], StatementNode::Declaration(_, _, ExpressionNode::Literal(Type::Long(t)), _) if t.text == "42")
    );
    assert!(
        matches!(&body[1], StatementNode::Declaration(_, _, ExpressionNode::Literal(Type::UInt(t)), _) if t.text == "7")
    );
    assert!(
        matches!(&body[2], StatementNode::Declaration(_, _, ExpressionNode::Literal(Type::ULong(t)), _) if t.text == "9")
    );
    assert!(
        matches!(&body[3], StatementNode::Declaration(_, _, ExpressionNode::Literal(Type::Byte(t)), _) if t.text == "255")
    );
}

#[test]
fn test_parse_error_recovery() {
    let code = "fun test(): void { let x = ; let y = 5; }";
    let arena = bumpalo::Bump::new();
    let (_, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), true);
    // The parser should report an error for the missing expression but continue parsing `let y = 5;`
    assert!(!diagnostics.diagnostics.is_empty());
}

#[test]
fn test_parse_nested_generic_type_annotation() {
    // Nested generics close with `>>` (a single ShiftRight token); the parser must split it.
    let code = "fun main(): void { let b: Box<Box<int>> = null; }";
    let arena = bumpalo::Bump::new();
    let (_, diagnostics) = parse_code(code, &arena);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_parse_multi_arg_nested_generic_instantiation() {
    // `Pair<Box<int>, int>(...)` must be recognized as a (constructor) call despite the
    // nested generic in the first type argument.
    let code = "class Box<T> { v: T; } class Pair<A, B> { first: A; second: B; } \
                fun main(): void { let p = Pair<Box<int>, int>(Box<int>(1), 2); }";
    let arena = bumpalo::Bump::new();
    let (_, diagnostics) = parse_code(code, &arena);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_parse_struct_comma_fields_recovers_without_hanging() {
    // Comma-separated class fields are invalid (fields use ';'). The parser must report an
    // error and terminate rather than spin forever on the unexpected token.
    let code = "class Point { x: int, y: int, } fun main(): void { }";
    let arena = bumpalo::Bump::new();
    let (_, diagnostics) = parse_code(code, &arena);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_parse_struct_constructor_and_destructor() {
    // `constructor(...)` and `del()` parse as class methods named `constructor` / `del`
    // without the `fun` keyword or a return type.
    let code = "class Point { x: int; y: int; \
                constructor(x: int, y: int) { this.x = x; this.y = y; } \
                del() { } \
                fun sum(): int { return this.x + this.y; } }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.structs.len(), 1);
    let s = &program.structs[0];
    assert_eq!(s.fields.len(), 2);

    let init = s
        .methods
        .iter()
        .find(|m| m.name.text == "constructor")
        .expect("constructor method");
    assert_eq!(init.parameters.len(), 2);
    assert!(init.return_type.is_none());

    let drop = s
        .methods
        .iter()
        .find(|m| m.name.text == "del")
        .expect("del method");
    assert_eq!(drop.parameters.len(), 0);
    assert!(drop.return_type.is_none());

    assert!(s.methods.iter().any(|m| m.name.text == "sum"));
}

#[test]
fn test_parse_struct_keyword_sets_is_value() {
    // The `struct` keyword parses through the same path as `class` but flags the declaration as a
    // value type (`is_value`); a `class` declaration leaves it `false`.
    let code = "struct Vec2 { x: int; y: int; \
                constructor(x: int, y: int) { this.x = x; this.y = y; } \
                fun sum(): int { return this.x + this.y; } } \
                class Ref { v: int; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.structs.len(), 2);

    let value = program
        .structs
        .iter()
        .find(|s| s.name.text == "Vec2")
        .expect("Vec2 declaration");
    assert!(value.is_value, "`struct` must set is_value = true");
    assert_eq!(value.fields.len(), 2);
    assert!(value.methods.iter().any(|m| m.name.text == "sum"));

    let reference = program
        .structs
        .iter()
        .find(|s| s.name.text == "Ref")
        .expect("Ref declaration");
    assert!(!reference.is_value, "`class` must leave is_value = false");
}

#[test]
fn test_parse_async_function_and_await() {
    // `async fun` sets `is_async`; `await e;` is an `AwaitStmt` and `let x = await e;` carries an
    // `Await` initializer.
    let code = "async fun f(): int { await sleep(1); let x = await f(); return x; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    assert!(func.is_async);
    assert!(matches!(&func.body[0], StatementNode::AwaitStmt(_)));
    assert!(matches!(
        &func.body[1],
        StatementNode::Declaration(_, _, ExpressionNode::Await(_), _)
    ));
}

#[test]
fn test_parse_extern_async_either_order() {
    // Both `extern async fun` and `async extern fun` parse to an async extern import.
    for code in [
        "extern async fun g(id: int): string;",
        "async extern fun g(id: int): string;",
    ] {
        let arena = bumpalo::Bump::new();
        let (program, diagnostics) = parse_code(code, &arena);
        assert_eq!(diagnostics.has_errors(), false, "code: {}", code);
        let func = &program.functions[0];
        assert!(func.is_extern, "code: {}", code);
        assert!(func.is_async, "code: {}", code);
    }
}

// --- Property / fuzz tests --------------------------------------------------------------------
// The parser is a recover-and-continue recursive-descent parser: on *any* input it must report
// diagnostics rather than panic, and `parse()` must always succeed in producing a `ProgramNode`
// (it never returns `Err`). These tests throw large amounts of malformed input at it; reaching the
// end of each test (without a panic or hang) is the assertion.

/// Tiny deterministic xorshift PRNG so fuzz inputs are reproducible without external crates.
struct XorShift(u64);
impl XorShift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick<'t>(&mut self, items: &[&'t str]) -> &'t str {
        items[(self.next_u64() as usize) % items.len()]
    }
}

/// Parses `code` and asserts the parser produced a `ProgramNode` without panicking or erroring.
fn assert_parses_without_panic(code: &str) {
    let arena = bumpalo::Bump::new();
    let mut diagnostics = DiagnosticBag::new(None);
    let lexer = Lexer::new(code.to_string());
    let mut parser = Parser::new(lexer, &arena, &mut diagnostics);
    let result = parser.parse();
    assert!(
        result.is_ok(),
        "parser returned Err (should always yield a ProgramNode) for input: {:?}",
        code
    );
}

#[test]
fn fuzz_random_token_soup_never_panics() {
    const TOKENS: [&str; 64] = [
        "fun",
        "class",
        "enum",
        "extend",
        "let",
        "const",
        "public",
        "static",
        "async",
        "return",
        "if",
        "else",
        "while",
        "for",
        "do",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "import",
        "type",
        "constructor",
        "del",
        "await",
        "true",
        "false",
        "null",
        "int",
        "string",
        "bool",
        "double",
        "float",
        "char",
        "void",
        "object",
        "{",
        "}",
        "(",
        ")",
        "[",
        "]",
        "<",
        ">",
        ":",
        ";",
        ",",
        ".",
        "=",
        "==",
        "+",
        "-",
        "*",
        "/",
        "%",
        "?",
        "@",
        "&&",
        "||",
        "\"s\"",
        "123",
        "3.14",
        "'c'",
        "ident",
    ];
    let mut rng = XorShift(0x9E3779B97F4A7C15);
    for _ in 0..3000 {
        let len = (rng.next_u64() as usize) % 40;
        let mut s = String::new();
        for _ in 0..len {
            s.push_str(rng.pick(&TOKENS));
            s.push(' ');
        }
        assert_parses_without_panic(&s);
    }
}

#[test]
fn fuzz_truncated_valid_programs_never_panic() {
    let samples = [
        "fun main(): int { return 42; }",
        "class Box<T> { public value: T; }",
        "public fun add(a: int, b: int): int { return a + b; }",
        "enum Color { Red, Green = 5, Blue }",
        "fun f() { let xs: int[] = [1,2,3]; for (x in xs) { System.println(x); } }",
        "extend int { public fun doubled(): int { return this * 2; } }",
        "const LIMIT: int = 5; let counter: int = LIMIT * 2;",
        "@json class User { public name: string; public age: int; }",
        "async fun g(): int { await sleep(1); return await h(); }",
    ];
    for s in samples {
        // Every byte prefix (a "file cut off mid-token") must still parse without panicking.
        for end in 0..=s.len() {
            if !s.is_char_boundary(end) {
                continue;
            }
            assert_parses_without_panic(&s[..end]);
        }
    }
}

#[test]
fn fuzz_byte_mutations_never_panic() {
    let base = "fun main(): int { let x = foo(1, 2); return x; }";
    let bytes = base.as_bytes();
    let mut rng = XorShift(0xDEAD_BEEF_CAFE_F00D);
    for _ in 0..3000 {
        let mut v = bytes.to_vec();
        let mutations = 1 + (rng.next_u64() as usize) % 6;
        for _ in 0..mutations {
            let idx = (rng.next_u64() as usize) % v.len();
            v[idx] = (rng.next_u64() as u8) | 0x20; // bias toward printable bytes
        }
        let s = String::from_utf8_lossy(&v);
        assert_parses_without_panic(&s);
    }
}

#[test]
fn fuzz_unbalanced_delimiters_never_panic() {
    let pieces = [
        "{", "}", "(", ")", "[", "]", "<", ">", "fun", "class", "if", "for", "x", ";", ":", ",",
    ];
    let mut rng = XorShift(0x1234_5678_ABCD_EF01);
    for _ in 0..4000 {
        let len = (rng.next_u64() as usize) % 60;
        let mut s = String::new();
        for _ in 0..len {
            s.push_str(rng.pick(&pieces));
        }
        assert_parses_without_panic(&s);
    }
}

#[test]
fn test_parse_interface_declaration() {
    let code = "interface Animal { fun Call(): string; fun Legs(): int; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.interfaces.len(), 1);
    let iface = &program.interfaces[0];
    assert_eq!(iface.name.text, "Animal");
    assert_eq!(iface.methods.len(), 2);
    assert_eq!(iface.methods[0].name.text, "Call");
    assert_eq!(iface.methods[1].name.text, "Legs");
    // Interface methods are body-less signatures.
    assert!(iface.methods[0].body.is_empty());
}

#[test]
fn test_parse_class_implements_clause() {
    let code = "class Cat : Animal, Pet { fun call(): string { return \"meow\"; } }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.structs.len(), 1);
    let cat = &program.structs[0];
    assert_eq!(cat.name.text, "Cat");
    let implemented: Vec<String> = cat.implements.iter().map(|t| t.get_type()).collect();
    assert_eq!(implemented, vec!["Animal".to_string(), "Pet".to_string()]);
}

#[test]
fn test_parse_class_implements_generic_interface() {
    let code = "class Box<T> : Container<int> { fun get(): int { return 0; } }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let boxc = &program.structs[0];
    assert_eq!(boxc.name.text, "Box");
    let implemented: Vec<String> = boxc.implements.iter().map(|t| t.display_name()).collect();
    assert_eq!(implemented, vec!["Container<int>".to_string()]);
}

#[test]
fn test_parse_is_with_binding() {
    let code = "fun f(o: object): void { if (o is int a) { print(a); } }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    // The `if` condition should be an `is` expression carrying a binding token.
    if let StatementNode::IfElse(cond, _, _, _) = &func.body[0] {
        if let ExpressionNode::IsExpression(_, ty, binding) = cond {
            assert!(matches!(ty, Type::Integer(_)));
            assert_eq!(binding.as_ref().map(|t| t.text.as_str()), Some("a"));
        } else {
            panic!("expected an IsExpression condition");
        }
    } else {
        panic!("expected an if statement");
    }
}

#[test]
fn test_parse_is_without_binding_still_works() {
    let code = "fun f(o: object): void { if (o is int) { print(1); } }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    if let StatementNode::IfElse(cond, _, _, _) = &func.body[0] {
        if let ExpressionNode::IsExpression(_, _, binding) = cond {
            assert!(binding.is_none());
        } else {
            panic!("expected an IsExpression condition");
        }
    } else {
        panic!("expected an if statement");
    }
}

#[test]
fn fuzz_recovers_and_reports_multiple_errors() {
    // Two independently-broken statements: a robust block parser should recover from the first
    // and still parse/lint the second (so we expect to keep the valid trailing statement).
    let code = "fun main(): void { let = ; let y = 5; @@@ ; return y; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);
    assert!(
        diagnostics.has_errors(),
        "malformed block should report diagnostics"
    );
    // The parser still produced a function (didn't discard the whole declaration).
    assert_eq!(
        program.functions.len(),
        1,
        "function should still be recovered"
    );
}

#[test]
fn test_parse_default_parameter_value() {
    let code = "fun f(x: int, y: int = 5): void {}";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    assert_eq!(func.parameters.len(), 2);
    assert!(func.parameters[0].default.is_none());
    match &func.parameters[1].default {
        Some(Type::Integer(t)) => assert_eq!(t.text, "5"),
        other => panic!("expected integer default `5`, got {:?}", other),
    }
}

#[test]
fn test_parse_negative_default_parameter_value() {
    let code = "fun f(z: int = -1): void {}";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    match &func.parameters[0].default {
        Some(Type::Integer(t)) => assert_eq!(t.text, "-1"),
        other => panic!("expected integer default `-1`, got {:?}", other),
    }
}

#[test]
fn test_parse_string_and_bool_default_parameter_values() {
    let code = "fun f(name: string = \"anon\", flag: bool = true): void {}";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    let func = &program.functions[0];
    assert!(matches!(func.parameters[0].default, Some(Type::String(_))));
    assert!(matches!(func.parameters[1].default, Some(Type::Boolean(_))));
}

#[test]
fn test_required_parameter_after_default_is_rejected() {
    // A required parameter following one with a default must be reported as an error.
    let code = "fun f(x: int = 1, y: int): void {}";
    let arena = bumpalo::Bump::new();
    let (_program, diagnostics) = parse_code(code, &arena);

    assert!(
        diagnostics.has_errors(),
        "a required parameter after a defaulted one should be a parse error"
    );
}

/// Extracts the single expression from `let _ = <expr>;` inside `main`, for cast-parsing tests.
fn only_decl_expr<'a>(program: &'a ProgramNode<'a>) -> &'a ExpressionNode<'a> {
    let func = &program.functions[0];
    match &func.body[0] {
        StatementNode::Declaration(_, _, expr, _) => expr,
        other => panic!("expected a declaration, got {:?}", other),
    }
}

#[test]
fn test_parse_cast_with_generic_type_argument() {
    let code = "fun main(): void { let c = (Container<int>)b; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    match only_decl_expr(&program) {
        ExpressionNode::Cast(Type::Struct(name, Some(args)), _) => {
            assert_eq!(name.text, "Container");
            assert_eq!(args.len(), 1);
            assert!(matches!(args[0], Type::Integer(_)));
        }
        other => panic!("expected a cast to Container<int>, got {:?}", other),
    }
}

#[test]
fn test_parse_cast_with_nested_generic_type_argument() {
    let code = "fun main(): void { let x = (Pair<Box<int>, int>)value; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    match only_decl_expr(&program) {
        ExpressionNode::Cast(Type::Struct(name, Some(args)), _) => {
            assert_eq!(name.text, "Pair");
            assert_eq!(args.len(), 2);
        }
        other => panic!("expected a cast to Pair<Box<int>, int>, got {:?}", other),
    }
}

#[test]
fn test_parse_dotted_import() {
    let code = "import a.b.c;\nfun main(): void {}";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.imports.len(), 1);
    assert_eq!(program.imports[0].module_name.text, "a/b/c");
}

#[test]
fn test_parse_single_segment_import() {
    let code = "import math_lib;\nfun main(): void {}";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    assert_eq!(program.imports.len(), 1);
    assert_eq!(program.imports[0].module_name.text, "math_lib");
}

#[test]
fn test_parenthesized_comparison_is_not_a_cast() {
    // `(x) < y` is a comparison, not a `(Type)expr` cast: the generic lookahead must not
    // misclassify it. It should parse as a parenthesized expression on the left of a `<`.
    let code = "fun main(): void { let r = (x) < y; }";
    let arena = bumpalo::Bump::new();
    let (program, diagnostics) = parse_code(code, &arena);

    assert_eq!(diagnostics.has_errors(), false);
    match only_decl_expr(&program) {
        ExpressionNode::Binary(_, opr, _) => assert_eq!(opr.kind, TokenKind::SmallerThanToken),
        other => panic!("expected a `<` comparison, got {:?}", other),
    }
}
