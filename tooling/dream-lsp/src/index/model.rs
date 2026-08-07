//! Symbol-model data types shared across the index: declaration/reference records, inlay-hint
//! payloads, and the small rendering helpers used when building and querying the model.

use dream::syntax::nodes::types::{CONSTRUCTOR_NAME, DESTRUCTOR_NAME};
use dream::syntax::nodes::{FunctionNode, Type};

/// Sentinel scope id for declarations that live at file scope (functions, structs, enums).
pub(crate) const GLOBAL: usize = usize::MAX;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymKind {
    Function,
    /// A reference-counted `class` type.
    Class,
    /// A value-type `struct` (including `ref struct`).
    Struct,
    /// An `interface` type.
    Interface,
    Enum,
    EnumMember,
    Field,
    Method,
    Variable,
    Param,
    Type,
    Keyword,
    /// An `@attribute` name (`@json`, `@get_indexer`, …).
    Decorator,
    /// An importable package or module path (`system.collections`, local `.dream`).
    Module,
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
    /// For a field/method/enum-member reference (`recv.name`), the receiver's identifier text
    /// (e.g. `obj` in `obj.field`, `Color` in `Color.Red`), when the receiver is a plain
    /// identifier. Captured from the AST at index-build time so hover/go-to-definition never need
    /// to re-derive the receiver by scanning source bytes backwards from the reference.
    pub receiver: Option<String>,
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
        Type::Array(inner) => base_struct(inner),
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

/// Renders a free-function declaration's signature, e.g. `fun add(a: int, b: int): int`
/// or `async fun f(): int`.
pub(crate) fn signature(func: &FunctionNode) -> String {
    let params = param_list(func);
    let ret = return_type_str(func);

    let prefix = if func.is_async { "async fun " } else { "fun " };

    if func.name.text == CONSTRUCTOR_NAME || func.name.text == DESTRUCTOR_NAME {
        format!("{}({}): {}", func.name.text, params, ret)
    } else {
        format!("{}{}({}): {}", prefix, func.name.text, params, ret)
    }
}

/// Renders a method/field-owner detail for the index, e.g.
/// `static ComputePass.begin(): ComputePass`,
/// `ComputePass.dispatch(kernel: string, …): void`, or
/// `async WebWorkerPool.dispatch<TIn, TOut>(msg: TIn, …): TOut`.
pub(crate) fn method_detail(owner: &str, func: &FunctionNode) -> String {
    let params = param_list(func);
    let ret = return_type_str(func);
    let generics = type_params_str(func);
    let static_prefix = if func.is_static { "static " } else { "" };
    let async_prefix = if func.is_async { "async " } else { "" };

    if func.name.text == CONSTRUCTOR_NAME || func.name.text == DESTRUCTOR_NAME {
        format!(
            "{static_prefix}{async_prefix}{owner}.{}({}): {}",
            func.name.text, params, ret
        )
    } else {
        format!(
            "{static_prefix}{async_prefix}{owner}.{}{generics}({}): {}",
            func.name.text, params, ret
        )
    }
}

/// True when a method detail string was indexed as a `static` method.
pub(crate) fn detail_is_static_method(detail: &str) -> bool {
    detail.starts_with("static ")
}

fn param_list(func: &FunctionNode) -> String {
    func.parameters
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
        .join(", ")
}

fn return_type_str(func: &FunctionNode) -> String {
    func.return_type
        .as_ref()
        .map(|t| t.display_name())
        .unwrap_or_else(|| "void".to_string())
}

fn type_params_str(func: &FunctionNode) -> String {
    func.generic_parameters
        .as_ref()
        .map(|params| {
            let names = params
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("<{names}>")
        })
        .unwrap_or_default()
}

/// Strips `?` / `[]` suffixes and takes the bare type name before `<…>` (e.g. `List<int>` → `List`).
pub(crate) fn type_base(ty: &str) -> &str {
    let base = ty.trim_end_matches('?').trim_end_matches("[]").trim();
    base.split('<').next().unwrap_or(base).trim()
}

/// Substitutes method type parameters in a detail string when call-site type args are known.
/// `detail` looks like `async WebWorkerPool.dispatch<TIn, TOut>(msg: TIn, …): TOut`;
/// `type_args` are the call-site args (`["int", "string"]`).
pub(crate) fn substitute_method_type_args(detail: &str, type_args: &[String]) -> String {
    if type_args.is_empty() {
        return detail.to_string();
    }
    // Type params sit in the first `<…>` after the method name and before `(`.
    let Some(paren) = detail.find('(') else {
        return detail.to_string();
    };
    let head = &detail[..paren];
    let Some(lt) = head.rfind('<') else {
        return detail.to_string();
    };
    let Some(gt) = head[lt..].find('>') else {
        return detail.to_string();
    };
    let params_str = &head[lt + 1..lt + gt];
    let params: Vec<&str> = params_str.split(',').map(|s| s.trim()).collect();
    if params.is_empty() {
        return detail.to_string();
    }

    let mut out = detail.to_string();
    // Drop the `<TIn, TOut>` clause from the displayed name once substituted.
    let generics_span = format!("<{params_str}>");
    out = out.replacen(&generics_span, "", 1);

    for (param, arg) in params.iter().zip(type_args.iter()) {
        if param.is_empty() {
            continue;
        }
        // Same word-ish replacements used for receiver `T`, generalized to any param name.
        out = out
            .replace(&format!("<{param}>"), &format!("<{arg}>"))
            .replace(&format!(": {param}"), &format!(": {arg}"))
            .replace(&format!(" {param},"), &format!(" {arg},"))
            .replace(&format!(" {param})"), &format!(" {arg})"))
            .replace(&format!(" {param}>"), &format!(" {arg}>"))
            .replace(&format!(" {param} "), &format!(" {arg} "))
            .replace(&format!("({param})"), &format!("({arg})"))
            .replace(&format!("({param},"), &format!("({arg},"))
            .replace(&format!(", {param},"), &format!(", {arg},"))
            .replace(&format!(", {param})"), &format!(", {arg})"))
            .replace(&format!("fun({param}):"), &format!("fun({arg}):"))
            .replace(&format!("fun({param})"), &format!("fun({arg})"));
        // Trailing bare return type `…: TOut` when TOut is the whole return.
        if out.ends_with(&format!(": {param}")) {
            out = format!("{}: {arg}", &out[..out.len() - param.len() - 2]);
        }
    }
    out
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
