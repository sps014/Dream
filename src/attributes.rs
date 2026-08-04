//! Central registry and validator for the `@name(args)` attribute syntax.
//!
//! Attribute *parsing* (`crates/dream-syntax/src/parser/declarations.rs::parse_attributes`) is,
//! and stays, fully generic: any `@identifier` or `@identifier(arg, ...)` parses on any
//! attribute-bearing declaration, with args stored as raw tokens. Historically every consumer
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

use crate::diagnostics::DiagnosticBag;
use crate::syntax::nodes::function::FunctionNode;
use crate::syntax::nodes::interface_node::InterfaceDeclarationNode;
use crate::syntax::nodes::program::{EnumDeclarationNode, ExtendNode};
use crate::syntax::nodes::struct_node::{StructDeclarationNode, StructFieldNode};
use crate::syntax::nodes::types::is_special_member_name;
use crate::syntax::nodes::AttributeNode;
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
        }
    }
}

/// The expected shape of an attribute's argument list.
#[derive(Debug, Clone, Copy)]
pub enum ArgShape {
    /// `@name` with no `(...)` at all, or empty parens.
    None,
    /// `@name("a", "b", ...)`: between `min` and `max` (inclusive) string-literal arguments.
    Strings { min: usize, max: usize },
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
        args: ArgShape::Strings { min: 1, max: 1 },
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
        args: ArgShape::Strings { min: 1, max: 1 },
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
        args: ArgShape::Strings { min: 2, max: 2 },
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
        args: ArgShape::Strings { min: 1, max: 1 },
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
        args: ArgShape::Strings { min: 1, max: 1 },
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
        // see `src/stdlib/core/sync.dream`). Rejecting `@shared struct` here means the rest of the
        // compiler never needs to reason about a value-typed shared class.
        targets: &[AttributeTarget::Struct],
        args: ArgShape::None,
        repeatable: false,
    },
    AttributeSpec {
        name: "stack",
        // A checked contract, not a request: every monomorphized instance of a `@stack` union
        // must already qualify as a value union (all payloads value/primitive, or a single
        // reference-typed payload) or registration reports an error. See
        // `Analyzer::register_union` in `src/semantics/analyzer/declarations/enums.rs`.
        targets: &[AttributeTarget::Union],
        args: ArgShape::None,
        repeatable: false,
    },
];

fn find_spec(name: &str) -> Option<&'static AttributeSpec> {
    ATTRIBUTES.iter().find(|s| s.name == name)
}

/// Validates one declaration's attribute list against `target`: every attribute must be a known
/// name, allowed on `target`, carry the right argument shape, and (unless `repeatable`) appear at
/// most once. Reports every violation it finds rather than stopping at the first, since each
/// attribute is independent.
pub fn validate_attributes(
    attrs: &[AttributeNode],
    target: AttributeTarget,
    diagnostics: &mut DiagnosticBag,
) {
    let mut seen: Vec<&str> = Vec::new();
    for attr in attrs {
        let name = attr.name.text.as_str();
        let Some(spec) = find_spec(name) else {
            diagnostics.report_error(
                format!("unknown attribute '@{}'", name),
                Some(attr.name.position),
            );
            continue;
        };

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
            ArgShape::Strings { min, max } => {
                if attr.args.len() < min || attr.args.len() > max {
                    let expected = if min == max {
                        format!("{}", min)
                    } else {
                        format!("{}-{}", min, max)
                    };
                    diagnostics.report_error(
                        format!(
                            "'@{}' expects {} string argument(s), got {}",
                            name,
                            expected,
                            attr.args.len()
                        ),
                        Some(attr.name.position),
                    );
                }
            }
        }

        if !spec.repeatable && seen.contains(&name) {
            diagnostics.report_error(
                format!("duplicate '@{}' attribute", name),
                Some(attr.name.position),
            );
        }
        seen.push(name);
    }
}

/// Extracts the `(module, field)` pair from a `@js("module", "field")` attribute, or `None` if the
/// declaration carries no `@js` attribute. `validate_program_attributes` already guarantees that a
/// present `@js` has exactly two string arguments, so this never needs to fall back on a partial
/// match. Single source of truth for the extraction previously duplicated between `driver/abi.rs`
/// and `semantics::analyzer::hir_emit`.
pub fn js_import_target(attributes: &[AttributeNode]) -> Option<(String, String)> {
    let js = attributes.iter().find(|a| a.name.text == "js")?;
    let module = js.args.first()?.text.trim_matches('"').to_string();
    let field = js.args.get(1)?.text.trim_matches('"').to_string();
    Some((module, field))
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
    diagnostics: &mut DiagnosticBag,
) {
    for f in functions {
        diagnostics.file_path = file_path_string(&f.file_path);
        let target = match function_target(f) {
            Some(AttributeTarget::Method) if top_level => AttributeTarget::Function,
            Some(t) => t,
            None => continue,
        };
        validate_attributes(&f.attributes, target, diagnostics);
    }
}

fn validate_fields(fields: &[StructFieldNode], diagnostics: &mut DiagnosticBag) {
    for field in fields {
        validate_attributes(&field.attributes, AttributeTarget::Field, diagnostics);
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
    enums: &[EnumDeclarationNode],
    extends: &[ExtendNode<'_>],
    diagnostics: &mut DiagnosticBag,
) {
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
        validate_attributes(&s.attributes, target, diagnostics);
        validate_fields(&s.fields, diagnostics);
        validate_function_list(&s.methods, false, diagnostics);
    }

    for i in interfaces {
        if i.file_path.is_none() {
            continue;
        }
        diagnostics.file_path = file_path_string(&i.file_path);
        validate_attributes(&i.attributes, AttributeTarget::Interface, diagnostics);
        for m in &i.methods {
            validate_attributes(&m.attributes, AttributeTarget::InterfaceMethod, diagnostics);
        }
    }

    validate_function_list(functions, true, diagnostics);

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
        validate_attributes(&e.attributes, target, diagnostics);
        for v in &e.variants {
            validate_fields(&v.fields, diagnostics);
        }
    }

    for ext in extends {
        if ext.is_synthesized {
            continue;
        }
        diagnostics.file_path = file_path_string(&ext.file_path);
        validate_function_list(&ext.methods, false, diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::token::syntax_token::SyntaxToken;
    use crate::syntax::token::token_kind::TokenKind;
    use crate::text::line_text::LineText;
    use crate::text::text_span::TextSpan;

    fn ident(text: &str) -> SyntaxToken {
        let span = TextSpan::new((0, 0), &LineText::new(String::new()));
        SyntaxToken::new(TokenKind::IdentifierToken, span, text.to_string())
    }

    fn attr(name: &str, args: &[&str]) -> AttributeNode {
        AttributeNode {
            name: ident(name),
            args: args.iter().map(|a| ident(a)).collect(),
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
