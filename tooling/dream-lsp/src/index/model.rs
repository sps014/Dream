//! Symbol-model data types shared across the index: declaration/reference records, inlay-hint
//! payloads, and the small rendering helpers used when building and querying the model.

use dream::syntax::nodes::types::{CONSTRUCTOR_NAME, DESTRUCTOR_NAME};
use dream::syntax::nodes::{FunctionNode, Type};

/// Sentinel scope id for declarations that live at file scope (functions, structs, enums).
pub(crate) const GLOBAL: usize = usize::MAX;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymKind {
    Function,
    Struct,
    Enum,
    EnumMember,
    Field,
    Method,
    Variable,
    Param,
    Type,
    Keyword,
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub name: String,
    pub kind: SymKind,
    /// The signature or type detail (e.g. `fun foo()` or `let x: int`).
    pub detail: String,
    /// Markdown-ready doc comment extracted from trivia.
    pub doc_comment: Option<String>,
    pub start: usize,
    pub end: usize,
    /// Function scope id, or [`GLOBAL`] for file-scope declarations.
    pub scope: usize,
    /// Resolved type name for variables/params/fields, used to type member access.
    pub ty: Option<String>,
    pub is_main: bool,
}

#[derive(Debug, Clone)]
pub struct Ref {
    pub name: String,
    pub kind: SymKind,
    pub start: usize,
    pub end: usize,
    pub scope: usize,
    pub is_main: bool,
}

/// Distinguishes an inferred-type hint (rendered after a `let` name, e.g. `: int`) from a
/// parameter-name hint (rendered before a call argument, e.g. `x:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlayKind {
    Type,
    Parameter,
}

/// A single inlay hint: where to anchor it (byte offset), its label, and what kind it is (which
/// drives padding/placement in the LSP layer).
#[derive(Debug, Clone)]
pub struct InlayHintOut {
    pub offset: usize,
    pub label: String,
    pub kind: InlayKind,
}
/// A located definition or reference (byte span + hover text).
pub struct Located {
    pub start: usize,
    pub end: usize,
    pub contents: String,
}
/// Returns the innermost struct type backing `ty` (peeling arrays and nullables), if any.
pub(crate) fn base_struct(ty: &Type) -> &Type {
    match ty {
        Type::Array(inner) | Type::Nullable(inner) => base_struct(inner),
        other => other,
    }
}

/// The parameter names of a function/method in declaration order (the implicit method `this` is
/// not a parsed parameter, so it never appears here).
pub(crate) fn param_names(func: &FunctionNode) -> Vec<String> {
    func.parameters
        .iter()
        .map(|p| p.name.text.clone())
        .collect()
}

/// Renders a function's *value* type, e.g. `fun(int, int): int` — used as the inferred type when
/// a function name is used as a value (`let a = fib;`), matching the `fun(ParamTypes): ReturnType`
/// syntax for first-class function types.
pub(crate) fn fn_value_type(func: &FunctionNode) -> String {
    let params = func
        .parameters
        .iter()
        .map(|p| p.type_.display_name())
        .collect::<Vec<_>>()
        .join(", ");
    let ret = func
        .return_type
        .as_ref()
        .map(|t| t.display_name())
        .unwrap_or_else(|| "void".to_string());
    format!("fun({}): {}", params, ret)
}

/// Renders a function declaration's signature, e.g. `fun add(a: int, b: int): int`.
pub(crate) fn signature(func: &FunctionNode) -> String {
    let params = func
        .parameters
        .iter()
        .map(|p| {
            if let Some(def) = &p.default {
                format!(
                    "{}: {} = {}",
                    p.name.text,
                    p.type_.display_name(),
                    def.display_name()
                )
            } else {
                format!("{}: {}", p.name.text, p.type_.display_name())
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ret = func
        .return_type
        .as_ref()
        .map(|t| t.display_name())
        .unwrap_or_else(|| "void".to_string());

    let prefix = if func.is_async { "async fun " } else { "fun " };

    if func.name.text == CONSTRUCTOR_NAME || func.name.text == DESTRUCTOR_NAME {
        format!("{}({}): {}", func.name.text, params, ret)
    } else {
        format!("{}{}({}): {}", prefix, func.name.text, params, ret)
    }
}

pub(crate) fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

/// Language keywords offered as completion proposals: every reserved word (`KEYWORDS`) plus the
/// contextual keywords that are only reserved in specific positions (`CONTEXTUAL_KEYWORDS`, e.g.
/// `this`/`get`/`set`/`constructor`/`del`). Re-exported from `dream-syntax` (the parser's own
/// source of truth) rather than hand-duplicated here, which is what previously let this list drift
/// out of sync with the parser (missing `interface`/`async`/`await`/`sealed`/`struct`/`unmanaged`).
pub fn keywords() -> impl Iterator<Item = &'static str> {
    dream::syntax::token::token_kind::KEYWORDS
        .iter()
        .chain(dream::syntax::token::token_kind::CONTEXTUAL_KEYWORDS)
        .copied()
}
