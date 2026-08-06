//! Central registry and validator for the `@name(args)` attribute syntax.
//!
//! Attribute *parsing* (`crates/dream-syntax/src/parser/declarations/attributes.rs`) is,
//! and stays, fully generic: any `@identifier` or `@identifier(arg, ...)` parses on any
//! attribute-bearing declaration, with args classified as typed [`AttributeArg`] constants
//! (string/int/float/double/bool/enum path). Historically every consumer
//! (`@intrinsic`, `@json`, `@property_name`, `@override`, `@js`, `@allow_cycle`) then hand-rolled
//! its own `attributes.iter().any(|a| a.name.text == "...")` check with no shared validation, so an
//! unknown attribute name (a typo like `@josn`) or a misapplied one (`@json` on a function) was
//! silently accepted and simply had no effect.
//!
//! This module is the single place that knows the full set of attribute names the compiler
//! recognizes, which kinds of declarations each may appear on, and what shape its arguments must
//! take. [`validate_program_attributes`] walks every attribute-bearing declaration once (called
//! from the driver, before semantic analysis) and reports unknown names, disallowed placements,
//! wrong argument counts, and (for non-repeatable attributes) duplicates. Attribute-specific
//! *meaning* (e.g. `@override` may only target `to_string`/`hash_code`, `@operator` must resolve
//! to a known operator symbol) is layered on top by each feature's own code, which can then assume
//! the generic shape/placement contract already holds.

use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::function::FunctionNode;
use dream_syntax::nodes::interface_node::InterfaceDeclarationNode;
use dream_syntax::nodes::program::{EnumDeclarationNode, ExtendNode};
use dream_syntax::nodes::struct_node::{StructDeclarationNode, StructFieldNode};
use dream_syntax::nodes::types::is_special_member_name;
use dream_syntax::nodes::{AttributeArg, AttributeNode, Type};
use std::collections::BTreeMap;
use std::rc::Rc;

/// The kind of declaration an attribute is attached to, coarse enough to express every current
/// placement rule (`@json` on a type, `@override` on an instance method, ...) without needing the
/// full declaration AST at validation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeTarget {
    /// A top-level, non-`extern` function.
    Function,
    /// A non-`static`, non-`extern` method on a `class`/`struct`/`extend` block.
    Method,
    /// A `static`, non-`extern` method on a `class`/`struct`/`extend` block.
    StaticMethod,
    /// Any function or method (top-level, instance, or static) declared `extern`.
    ExternFunction,
    /// A field on a `class`/`struct`/enum-variant payload.
    Field,
    /// A reference-type (`class`) declaration.
    Struct,
    /// A value-type (`struct`) declaration.
    ValueStruct,
    /// A plain C-style `enum` (no variant carries a payload).
    PlainEnum,
    /// A discriminated union (an `enum` where at least one variant carries a payload).
    Union,
    /// An `interface` declaration.
    Interface,
    /// A method signature inside an `interface`.
    InterfaceMethod,
    /// A file-level `module` declaration.
    Module,
}

impl AttributeTarget {
    fn display_name(self) -> &'static str {
        match self {
            AttributeTarget::Function => "a top-level function",
            AttributeTarget::Method => "an instance method",
            AttributeTarget::StaticMethod => "a static method",
            AttributeTarget::ExternFunction => "an extern function/method",
            AttributeTarget::Field => "a field",
            AttributeTarget::Struct => "a class",
            AttributeTarget::ValueStruct => "a struct",
            AttributeTarget::PlainEnum => "a plain enum",
            AttributeTarget::Union => "a discriminated union",
            AttributeTarget::Interface => "an interface",
            AttributeTarget::InterfaceMethod => "an interface method",
            AttributeTarget::Module => "a module declaration",
        }
    }
}

/// Kind of a single attribute argument — the attribute registry's analogue of a C# attribute
/// constructor parameter type. Declared once on the [`AttributeSpec`]; instances don't invent shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// A string literal (`"module"`, `"field"`, …).
    String,
    /// An integer literal (`8`, `64`, …).
    Int,
    /// An unsuffixed or `f`-suffixed float literal (`3.14`, `1.0f`).
    Float,
    /// A `d`-suffixed double literal (`3.14d`).
    Double,
    /// A boolean literal (`true` / `false`).
    Bool,
    /// A dotted enum-member path (`HttpMethod.Get`).
    Enum,
}

/// The expected shape of an attribute's argument list — the closed-world "constructor signature"
/// for builtin attributes. User-defined `@attribute` functions supply their schema from the
/// function parameters instead.
#[derive(Debug, Clone, Copy)]
pub enum ArgShape {
    /// `@name` with no `(...)` at all, or empty parens.
    None,
    /// `@name(...)` with between `min` and `max` (inclusive) arguments.
    /// Argument `i` must match `kinds[i.min(kinds.len() - 1)]` (so a single-kind slice covers
    /// variadic same-typed args like `@compute(8, 8)`).
    Args {
        kinds: &'static [ArgKind],
        min: usize,
        max: usize,
    },
}

/// One attribute's full contract: its name, the declaration kinds it may appear on, its argument
/// shape, and whether it may be repeated on the same declaration.
pub struct AttributeSpec {
    pub name: &'static str,
    pub targets: &'static [AttributeTarget],
    pub args: ArgShape,
    pub repeatable: bool,
}

/// Every attribute name the compiler recognizes. Adding a new attribute means adding one entry
/// here; [`validate_program_attributes`] then enforces its placement/shape everywhere, and the
/// feature module only needs to implement attribute-specific *meaning* on top.
pub const ATTRIBUTES: &[AttributeSpec] = &[
    AttributeSpec {
        name: "intrinsic",
        targets: &[AttributeTarget::ExternFunction],
        args: ArgShape::Args { kinds: &[ArgKind::String], min: 1, max: 1 },
        repeatable: false,
    },
    AttributeSpec {
        name: "json",
        targets: &[
            AttributeTarget::Struct,
            AttributeTarget::ValueStruct,
            AttributeTarget::Union,
        ],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "property_name",
        targets: &[AttributeTarget::Field],
        args: ArgShape::Args { kinds: &[ArgKind::String], min: 1, max: 1 },
        repeatable: false,
    },
    AttributeSpec {
        name: "json_ignore",
        targets: &[AttributeTarget::Field],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "override",
        targets: &[AttributeTarget::Method],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "js",
        targets: &[AttributeTarget::ExternFunction],
        args: ArgShape::Args { kinds: &[ArgKind::String, ArgKind::String], min: 2, max: 2 },
        repeatable: false,
    },
    AttributeSpec {
        name: "allow_cycle",
        targets: &[AttributeTarget::Struct],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "operator",
        targets: &[AttributeTarget::Method],
        args: ArgShape::Args { kinds: &[ArgKind::String], min: 1, max: 1 },
        // Not repeatable *on one method* (`@operator("+") @operator("-")` on the same method makes
        // no sense — a method implements exactly one operator). A struct declaring many distinct
        // `@operator`-tagged *methods* is fine: `validate_attributes` runs once per method, so this
        // only rejects stacking the same attribute twice on a single declaration.
        // `validate_and_register_operator` (`declarations::operator_overloads`) separately enforces
        // that no two methods on the same type claim the same operator symbol/arity.
        repeatable: false,
    },
    AttributeSpec {
        name: "cast",
        targets: &[AttributeTarget::Method],
        args: ArgShape::Args { kinds: &[ArgKind::String], min: 1, max: 1 },
        repeatable: false,
    },
    AttributeSpec {
        name: "unsafe",
        // Gates manual-memory-management operations (raw `Pointer<T>`): calling an `@unsafe`
        // function/method is only permitted from another `@unsafe` function/method — checked at
        // every call site, not just here at the declaration (see
        // `FunctionTableInfo::is_unsafe`/`Analyzer::check_unsafe_call`).
        targets: &[
            AttributeTarget::Function,
            AttributeTarget::Method,
            AttributeTarget::StaticMethod,
            AttributeTarget::ExternFunction,
        ],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "shared",
        // `class` only: a `struct` is a value type (copied on assignment, no heap allocation), so
        // an embedded lock word would defeat the point (`Shared<T>` is the value-type equivalent —
        // see `src/stdlib/system/core/lock.dream`). Rejecting `@shared struct` here means the rest of the
        // compiler never needs to reason about a value-typed shared class.
        targets: &[AttributeTarget::Struct],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "stack",
        // A checked contract, not a request: every monomorphized instance of a `@stack` union
        // must qualify as a value union (all payloads value/primitive, or any number of
        // non-self-referential reference-typed payloads) or registration reports an error. See
        // `Analyzer::register_union` in `src/semantics/analyzer/declarations/enums.rs`.
        targets: &[AttributeTarget::Union],
        args: ArgShape::None,
        repeatable: false,
    },
    // Indexer / enumerator protocol hooks (mirrors `@operator`: the attribute marks the role;
    // the method name is free). Shape/arity are checked in
    // `declarations::protocol_hooks::validate_and_register_protocol_hook`.
    AttributeSpec {
        name: "get",
        targets: &[AttributeTarget::Method, AttributeTarget::InterfaceMethod],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "set",
        targets: &[AttributeTarget::Method],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "iterator",
        targets: &[AttributeTarget::Method, AttributeTarget::InterfaceMethod],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "next",
        targets: &[AttributeTarget::Method, AttributeTarget::InterfaceMethod],
        args: ArgShape::None,
        repeatable: false,
    },
    // Source-generator framework (`system.codegen` / `driver/generate`).
    AttributeSpec {
        name: "generator",
        targets: &[
            AttributeTarget::Function,
            AttributeTarget::Method,
            AttributeTarget::StaticMethod,
        ],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "syntax_block",
        targets: &[
            AttributeTarget::Function,
            AttributeTarget::Method,
            AttributeTarget::StaticMethod,
        ],
        args: ArgShape::Args {
            kinds: &[ArgKind::String],
            min: 1,
            max: 1,
        },
        repeatable: true,
    },
    // User-defined attribute: `@attribute` on a bare top-level function; the function name is the
    // attribute name (exact casing), and its parameters are the `@name(...)` arg schema.
    AttributeSpec {
        name: "attribute",
        targets: &[AttributeTarget::Function],
        args: ArgShape::None,
        repeatable: false,
    },
    // WebGPU compute kernels: body is emitted as WGSL, not WASM. Optional 1–3 int args are the
    // workgroup size (X[, Y[, Z]]); bare `@compute` defaults to (64, 1, 1).
    AttributeSpec {
        name: "compute",
        targets: &[AttributeTarget::Function],
        args: ArgShape::Args {
            kinds: &[ArgKind::Int],
            min: 0,
            max: 3,
        },
        repeatable: false,
    },
];

fn find_spec(name: &str) -> Option<&'static AttributeSpec> {
    ATTRIBUTES.iter().find(|s| s.name == name)
}

fn arg_matches_kind(arg: &AttributeArg, kind: ArgKind) -> bool {
    match (kind, arg) {
        (ArgKind::String, AttributeArg::String(_)) => true,
        (ArgKind::Int, AttributeArg::Int(_)) => true,
        (ArgKind::Float, AttributeArg::Float(_)) => true,
        // Allow unsuffixed float literal when a double param is expected (same as expression
        // expected-type retargeting for numeric literals).
        (ArgKind::Double, AttributeArg::Double(_) | AttributeArg::Float(_)) => true,
        (ArgKind::Bool, AttributeArg::Bool(_)) => true,
        (ArgKind::Enum, AttributeArg::Enum(_)) => true,
        _ => false,
    }
}

fn kind_name(kind: ArgKind) -> &'static str {
    match kind {
        ArgKind::String => "a string literal",
        ArgKind::Int => "an integer literal",
        ArgKind::Float => "a float literal",
        ArgKind::Double => "a double literal",
        ArgKind::Bool => "a boolean literal",
        ArgKind::Enum => "an enum member path",
    }
}

fn type_to_arg_kind(ty: &Type) -> Option<ArgKind> {
    match ty {
        Type::String(_) => Some(ArgKind::String),
        Type::Integer(_) | Type::Byte(_) | Type::Long(_) | Type::UInt(_) | Type::ULong(_) => {
            Some(ArgKind::Int)
        }
        Type::Float(_) => Some(ArgKind::Float),
        Type::Double(_) => Some(ArgKind::Double),
        Type::Boolean(_) => Some(ArgKind::Bool),
        // Bare named types in params are treated as enum (e.g. `HttpMethod`).
        Type::Struct(_, None) | Type::Generic(_) => Some(ArgKind::Enum),
        _ => None,
    }
}

/// Top-level functions marked `@attribute`: name (exact casing) → parameter kinds.
fn collect_user_attributes(
    functions: &[FunctionNode<'_>],
    diagnostics: &mut DiagnosticBag,
) -> BTreeMap<String, Vec<ArgKind>> {
    let mut out = BTreeMap::new();
    for f in functions {
        if !f.attributes.iter().any(|a| a.name.text == "attribute") {
            continue;
        }
        diagnostics.file_path = file_path_string(&f.file_path);
        let mut kinds = Vec::new();
        let mut ok = true;
        for p in &f.parameters {
            match type_to_arg_kind(&p.type_) {
                Some(k) => kinds.push(k),
                None => {
                    diagnostics.report_error(
                        format!(
                            "attribute function '{}': parameter '{}' has a type that cannot be used as an attribute argument",
                            f.name.text, p.name.text
                        ),
                        Some(p.name.position),
                    );
                    ok = false;
                }
            }
        }
        if ok {
            if out.contains_key(&f.name.text) {
                diagnostics.report_error(
                    format!("duplicate attribute function '{}'", f.name.text),
                    Some(f.name.position),
                );
            } else {
                out.insert(f.name.text.clone(), kinds);
            }
        }
    }
    out
}

fn validate_arg_list(
    attr_name: &str,
    attr: &AttributeNode,
    kinds: &[ArgKind],
    min: usize,
    max: usize,
    diagnostics: &mut DiagnosticBag,
) {
    if attr.args.len() < min || attr.args.len() > max {
        let expected = if min == max {
            format!("{}", min)
        } else {
            format!("{}-{}", min, max)
        };
        diagnostics.report_error(
            format!(
                "'@{}' expects {} argument(s), got {}",
                attr_name,
                expected,
                attr.args.len()
            ),
            Some(attr.name.position),
        );
    }
    if kinds.is_empty() {
        return;
    }
    for (i, arg) in attr.args.iter().enumerate() {
        let kind = kinds[i.min(kinds.len() - 1)];
        if !arg_matches_kind(arg, kind) {
            diagnostics.report_error(
                format!(
                    "'@{}' argument {} must be {}, got '{}'",
                    attr_name,
                    i + 1,
                    kind_name(kind),
                    arg.display()
                ),
                Some(arg.position()),
            );
        }
    }
}

/// Validates one declaration's attribute list against `target`: every attribute must be a known
/// builtin name or a user `@attribute` function, allowed on `target`, carry the right argument shape,
/// and (unless `repeatable`) appear at most once.
pub fn validate_attributes(
    attrs: &[AttributeNode],
    target: AttributeTarget,
    diagnostics: &mut DiagnosticBag,
) {
    validate_attributes_with(attrs, target, &BTreeMap::new(), diagnostics);
}

fn validate_attributes_with(
    attrs: &[AttributeNode],
    target: AttributeTarget,
    user_attrs: &BTreeMap<String, Vec<ArgKind>>,
    diagnostics: &mut DiagnosticBag,
) {
    let mut seen: Vec<&str> = Vec::new();
    for attr in attrs {
        let name = attr.name.text.as_str();
        if let Some(spec) = find_spec(name) {
            if !spec.targets.contains(&target) {
                diagnostics.report_error(
                    format!("'@{}' cannot be applied to {}", name, target.display_name()),
                    Some(attr.name.position),
                );
            }
            match spec.args {
                ArgShape::None => {
                    if !attr.args.is_empty() {
                        diagnostics.report_error(
                            format!("'@{}' does not take any arguments", name),
                            Some(attr.name.position),
                        );
                    }
                }
                ArgShape::Args { kinds, min, max } => {
                    validate_arg_list(name, attr, kinds, min, max, diagnostics);
                }
            }
            if !spec.repeatable && seen.contains(&name) {
                diagnostics.report_error(
                    format!("duplicate '@{}' attribute", name),
                    Some(attr.name.position),
                );
            }
            seen.push(name);
            continue;
        }

        if let Some(kinds) = user_attrs.get(name) {
            let n = kinds.len();
            validate_arg_list(name, attr, kinds, n, n, diagnostics);
            if seen.contains(&name) {
                diagnostics.report_error(
                    format!("duplicate '@{}' attribute", name),
                    Some(attr.name.position),
                );
            }
            seen.push(name);
            continue;
        }

        diagnostics.report_error(
            format!("unknown attribute '@{}'", name),
            Some(attr.name.position),
        );
    }
}

/// Extracts the `(module, field)` pair from a `@js("module", "field")` attribute, or `None` if the
/// declaration carries no `@js` attribute. `validate_program_attributes` already guarantees that a
/// present `@js` has exactly two string arguments, so this never needs to fall back on a partial
/// match. Single source of truth for the extraction previously duplicated between `driver/abi.rs`
/// and `semantics::analyzer::hir_emit`.
pub fn js_import_target(attributes: &[AttributeNode]) -> Option<(String, String)> {
    let js = attributes.iter().find(|a| a.name.text == "js")?;
    let module = js.args.first()?.as_string()?.to_string();
    let field = js.args.get(1)?.as_string()?.to_string();
    Some((module, field))
}

/// True when the declaration carries `@compute`.
pub fn has_compute_attr(attributes: &[AttributeNode]) -> bool {
    attributes.iter().any(|a| a.name.text == "compute")
}

/// Workgroup size from `@compute` / `@compute(x[, y[, z]])`. Defaults to `(64, 1, 1)`.
pub fn compute_workgroup_size(attributes: &[AttributeNode]) -> (u32, u32, u32) {
    let Some(attr) = attributes.iter().find(|a| a.name.text == "compute") else {
        return (64, 1, 1);
    };
    let parse = |i: usize| -> u32 {
        attr.args
            .get(i)
            .and_then(|t| t.as_int_text())
            .and_then(|s| s.parse().ok())
            .unwrap_or(if i == 0 { 64 } else { 1 })
    };
    match attr.args.len() {
        0 => (64, 1, 1),
        1 => (parse(0), 1, 1),
        2 => (parse(0), parse(1), 1),
        _ => (parse(0), parse(1), parse(2)),
    }
}

fn file_path_string(file_path: &Option<Rc<str>>) -> Option<String> {
    file_path.as_ref().map(|p| p.to_string())
}

/// The target kind for a function/method declaration, derived from its own modifiers. `None` for
/// constructors/destructors and property accessors, which cannot carry attributes today (no known
/// attribute applies to them) and are skipped by the walk below.
fn function_target(f: &FunctionNode<'_>) -> Option<AttributeTarget> {
    if is_special_member_name(&f.name.text) || f.accessor.is_some() {
        return None;
    }
    Some(if f.is_extern {
        AttributeTarget::ExternFunction
    } else if f.is_static {
        AttributeTarget::StaticMethod
    } else {
        AttributeTarget::Method
    })
}

fn validate_function_list(
    functions: &[FunctionNode<'_>],
    top_level: bool,
    user_attrs: &BTreeMap<String, Vec<ArgKind>>,
    diagnostics: &mut DiagnosticBag,
) {
    for f in functions {
        diagnostics.file_path = file_path_string(&f.file_path);
        let target = match function_target(f) {
            Some(AttributeTarget::Method) if top_level => AttributeTarget::Function,
            Some(t) => t,
            None => continue,
        };
        validate_attributes_with(&f.attributes, target, user_attrs, diagnostics);
    }
}

fn validate_fields(
    fields: &[StructFieldNode],
    user_attrs: &BTreeMap<String, Vec<ArgKind>>,
    diagnostics: &mut DiagnosticBag,
) {
    for field in fields {
        validate_attributes_with(
            &field.attributes,
            AttributeTarget::Field,
            user_attrs,
            diagnostics,
        );
    }
}

/// Walks every attribute-bearing declaration in the (fully merged, pre-derive) program once,
/// reporting unknown/misapplied/malformed attributes. Run from the driver right after source
/// loading and prelude merge, before `@json` derivation and semantic analysis, so both of those
/// later stages can assume every attribute they see already has valid shape and placement.
/// Synthesized declarations (`file_path: None` for structs/enums/functions, or
/// `is_synthesized` for `extend` blocks) are compiler-generated and always skipped.
pub fn validate_program_attributes(
    structs: &[StructDeclarationNode<'_>],
    interfaces: &[InterfaceDeclarationNode<'_>],
    functions: &[FunctionNode<'_>],
    enums: &[EnumDeclarationNode<'_>],
    extends: &[ExtendNode<'_>],
    diagnostics: &mut DiagnosticBag,
) {
    let user_attrs = collect_user_attributes(functions, diagnostics);

    for s in structs {
        if s.file_path.is_none() {
            continue;
        }
        diagnostics.file_path = file_path_string(&s.file_path);
        let target = if s.is_value {
            AttributeTarget::ValueStruct
        } else {
            AttributeTarget::Struct
        };
        validate_attributes_with(&s.attributes, target, &user_attrs, diagnostics);
        validate_fields(&s.fields, &user_attrs, diagnostics);
        validate_function_list(&s.methods, false, &user_attrs, diagnostics);
    }

    for i in interfaces {
        if i.file_path.is_none() {
            continue;
        }
        diagnostics.file_path = file_path_string(&i.file_path);
        validate_attributes_with(
            &i.attributes,
            AttributeTarget::Interface,
            &user_attrs,
            diagnostics,
        );
        for m in &i.methods {
            validate_attributes_with(
                &m.attributes,
                AttributeTarget::InterfaceMethod,
                &user_attrs,
                diagnostics,
            );
        }
    }

    validate_function_list(functions, true, &user_attrs, diagnostics);

    for e in enums {
        if e.file_path.is_none() {
            continue;
        }
        diagnostics.file_path = file_path_string(&e.file_path);
        let target = if e.is_data_enum() {
            AttributeTarget::Union
        } else {
            AttributeTarget::PlainEnum
        };
        validate_attributes_with(&e.attributes, target, &user_attrs, diagnostics);
        for v in &e.variants {
            validate_fields(&v.fields, &user_attrs, diagnostics);
        }
        validate_function_list(&e.methods, false, &user_attrs, diagnostics);
    }

    for ext in extends {
        if ext.is_synthesized {
            continue;
        }
        diagnostics.file_path = file_path_string(&ext.file_path);
        validate_function_list(&ext.methods, false, &user_attrs, diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_syntax::nodes::AttributeArg;
    use dream_syntax::token::syntax_token::SyntaxToken;
    use dream_syntax::token::token_kind::TokenKind;
    use dream_text::line_text::LineText;
    use dream_text::text_span::TextSpan;

    fn ident(text: &str) -> SyntaxToken {
        let span = TextSpan::new((0, 0), &LineText::new(String::new()));
        SyntaxToken::new(TokenKind::IdentifierToken, span, text.to_string())
    }

    fn str_arg(text: &str) -> AttributeArg {
        let span = TextSpan::new((0, 0), &LineText::new(String::new()));
        AttributeArg::String(SyntaxToken::new(
            TokenKind::StringToken,
            span,
            text.to_string(),
        ))
    }

    fn attr(name: &str, args: &[&str]) -> AttributeNode {
        AttributeNode {
            name: ident(name),
            args: args.iter().map(|a| str_arg(a)).collect(),
        }
    }

    #[test]
    fn unknown_attribute_is_reported() {
        let mut diagnostics = DiagnosticBag::new(None);
        validate_attributes(
            &[attr("bogus", &[])],
            AttributeTarget::Method,
            &mut diagnostics,
        );
        assert!(diagnostics.has_errors());
    }

    #[test]
    fn misapplied_attribute_is_reported() {
        let mut diagnostics = DiagnosticBag::new(None);
        validate_attributes(
            &[attr("json", &[])],
            AttributeTarget::Function,
            &mut diagnostics,
        );
        assert!(diagnostics.has_errors());
    }

    #[test]
    fn wrong_arg_count_is_reported() {
        let mut diagnostics = DiagnosticBag::new(None);
        validate_attributes(
            &[attr("intrinsic", &[])],
            AttributeTarget::ExternFunction,
            &mut diagnostics,
        );
        assert!(diagnostics.has_errors());
    }

    #[test]
    fn duplicate_non_repeatable_attribute_is_reported() {
        let mut diagnostics = DiagnosticBag::new(None);
        validate_attributes(
            &[attr("override", &[]), attr("override", &[])],
            AttributeTarget::Method,
            &mut diagnostics,
        );
        assert!(diagnostics.has_errors());
    }

    #[test]
    fn well_formed_attribute_is_accepted() {
        let mut diagnostics = DiagnosticBag::new(None);
        validate_attributes(
            &[attr("intrinsic", &["\"print\""])],
            AttributeTarget::ExternFunction,
            &mut diagnostics,
        );
        assert!(!diagnostics.has_errors());
    }
}
