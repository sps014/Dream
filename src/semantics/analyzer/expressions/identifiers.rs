//! Identifier resolution (locals, globals, first-class function values) and the name→`Type` parser.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::Type;
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(super) fn analyze_identifier(
        &mut self,
        id: &SyntaxToken,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let r = match (*symbol_table).as_ref().borrow().get_symbol(id) {
            Ok(t) => t,
            Err(e) => {
                // A bare identifier that names a top-level function is a first-class function value.
                if let Ok(sig) = self.function_table.get_function(&id.text) {
                    // A boxed `fun(...)`-typed value is invoked through a plain synchronous
                    // `call_indirect` (no coroutine driver at the call site), so an `async fun`
                    // can never be soundly boxed this way in general — it would return before
                    // its body's awaits actually resolve. `WebWorker`/`.map`/`.dispatch`'s own
                    // body-invocation trampoline (`$__dream_worker_invoke`, `src/mir/emit/module.rs`)
                    // is the one exception: it recognizes an async constructor's untagged `Future`
                    // result and drives it to completion before returning, so a non-capturing
                    // top-level `async fun` is sound to pass there — reject it everywhere else,
                    // where no such driver exists and it would yield an unfinished/garbage result.
                    if sig.is_async {
                        // The trampoline's Future-tag check (see `src/mir/emit/module.rs`) only
                        // stays sound through `Bytes.toWire<TOut>`'s wrapping if `TOut` is `string`:
                        // that is the one case where `toWire` is a literal identity passthrough
                        // (`static_dispatch/intrinsics.rs`'s `WireEncode` arm), so the untagged
                        // `Future` pointer an async body's `call_indirect` actually returns reaches
                        // the trampoline unchanged. Any other `TOut` would instead byte-blit that
                        // raw pointer value as if it were already the real result — silent
                        // corruption, not a clean error - so require a `string`-returning body.
                        let returns_string = matches!(&sig.return_type, Some(t) if t.get_type() == "string");
                        if !self.is_webworker_body_call() || !returns_string {
                            return Err(report(
                                diagnostics,
                                format!(
                                    "'{}' is an async function and cannot be used as a first-class \
                                     `fun(...)` value - only a `WebWorker`/`WebWorker.map`/\
                                     `WebWorkerPool.dispatch` body may be async, and only if it is a \
                                     non-capturing top-level function returning `string` (not a \
                                     lambda, and not any other return type)",
                                    id.text
                                ),
                                Some(id.position),
                            ));
                        }
                    }
                    let params = sig
                        .parameters
                        .iter()
                        .map(|p| Self::type_from_name(p))
                        .collect();
                    let ret = sig.return_type.clone().unwrap_or(Type::Void);
                    let func_ty = Type::Function(params, Box::new(ret.clone()));
                    self.hir_set_func_value(&id.text, &func_ty, &ret);
                    return Ok(func_ty);
                }
                // A generic function used as a value (`let cmp: fun(T, T): int = natural_order;`):
                // infer its type arguments from the expected function type and instantiate it.
                if self.generic_functions.contains_key(&id.text) {
                    if let Some(func_ty) = self.instantiate_generic_function_value(id, diagnostics)
                    {
                        return Ok(func_ty);
                    }
                }
                // Unresolved name: report and short-circuit. Statement-level callers recover
                // (poisoning the binding with `Type::Unknown`) so sibling errors still surface.
                return Err(report(diagnostics, e.to_string(), Some(id.position)));
            }
        };
        // File/module-level visibility (Axis 2): a non-public top-level variable is only readable
        // from its declaring file. (Locals/params never appear in `self.globals`, so a shadowing
        // local of the same name is unaffected.)
        if let Some(global) = self.globals.iter().find(|g| g.name == id.text) {
            if !self.visible_across_files(
                &global.file_path,
                global.visibility,
                self.current_file.as_ref(),
            ) {
                let decl_file = global.file_path.clone();
                self.report_not_public("Variable", &id.text, &decl_file, id.position, diagnostics);
            }
        }
        self.hir_set_var(&id.text);
        Ok(r)
    }

    /// Reconstructs a `Type` from its canonical type-name string (as stored in function-table
    /// signatures), e.g. "int", "string", "Node", "int[]", "fun(int,int):int". Falls back to `void`
    /// if unparseable.
    pub(in crate::semantics::analyzer) fn type_from_name(name: &str) -> Type {
        // `Type::Function::get_type()` renders as `fun(<params joined by ",">):<ret>`, with no
        // spaces (see `types.rs`); reverse it here so a `fun(...)`-typed function-table parameter
        // (e.g. a `sort_by(cmp: fun(T, T): int)` parameter, or a synthesized lambda's own signature)
        // round-trips correctly instead of collapsing to a bogus struct type. Struct generic args
        // mangle to `_`-joined names (no `<`/`>`), so only `(`/`)` and `[`/`]` need nesting tracking.
        if let Some(rest) = name.strip_prefix("fun(") {
            if let Some(close) = matching_close_paren(rest) {
                let params_str = &rest[..close];
                if let Some(ret_str) = rest[close + 1..].strip_prefix(':') {
                    let params = split_top_level_commas(params_str)
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .map(|p| Self::type_from_name(&p))
                        .collect();
                    let ret = Self::type_from_name(ret_str);
                    return Type::Function(params, Box::new(ret));
                }
            }
        }
        let token = synthetic_token(TokenKind::IdentifierToken, name);
        Type::from_token(token).unwrap_or(Type::Void)
    }
}

/// Given the text immediately after a `fun(`'s opening paren, returns the byte index (into that
/// text) of the `)` that closes it, tracking `(`/`[` nesting so a nested `fun(...)` parameter or an
/// array type doesn't terminate the scan early.
fn matching_close_paren(s: &str) -> Option<usize> {
    let mut depth = 1i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits a `fun(...)` parameter-list string on top-level commas only, respecting `(`/`[` nesting
/// so a nested `fun(a,b):c` or array-typed parameter isn't split in the middle.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_string());
    parts
}
