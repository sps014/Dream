//! The `size()`/`char_at()`/`to_string()`/`hash_code()` object protocol available on every (or
//! every primitive/array) receiver, checked before falling through to normal instance dispatch.

use super::super::super::*;
use dream_diagnostics::DiagnosticBag;
use dream_abi::intrinsics;
use crate::errors::SemanticError;
use dream_syntax::nodes::types::mangle_generic;
use dream_syntax::nodes::{ExpressionNode, Type};
use dream_syntax::token::syntax_token::SyntaxToken;
use dream_syntax::token::token_kind::TokenKind;
use dream_types::method_fn;

impl<'a> Analyzer<'a> {
    /// Type-checks the builtin methods available on every (or every primitive/array) receiver:
    /// `size()`, `str.char_at(i)`, and the `to_string`/`hash_code` object protocol (a C-style enum's
    /// `to_string()` renders its variant name). Returns `Ok(Some(result_type))` when the call is a
    /// builtin (so the caller returns it) or `Ok(None)` to fall through to normal instance-method
    /// dispatch. A user-defined `to_string`/`hash_code` override yields `None` so the override is
    /// dispatched normally.
    pub(crate) fn analyze_builtin_method(
        &mut self,
        obj_type: &Type,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::super::AnalyzerContext<'a, '_>,
        receiver: &mut Option<dream_hir::HExpr>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Option<Type>, SemanticError> {
        // Default: clear `last` until a matching builtin re-fills it (`size` / `to_string` / …).
        self.hir_none();

        // `arr.size()` / `str.size()`: built-in element-count method on arrays and strings (the same
        // `size()` the stdlib `List`/`Map` expose, so every collection is queried the same way).
        if method.text == intrinsics::SIZE {
            let base = obj_type.get_type();
            if base.ends_with("[]") || base == "string" {
                if !params.is_empty() {
                    diagnostics.report_error(
                        format!("'size' takes no arguments, got {}", params.len()),
                        Some(method.position),
                    );
                }
                self.hir_set_array_len(receiver.take());
                return Ok(Some(Type::Integer(synthetic_token(
                    TokenKind::DataTypeToken,
                    "int",
                ))));
            }
        }

        // `str.char_at(i)`: built-in character accessor on strings (low-level read).
        if method.text == intrinsics::CHAR_AT && obj_type.get_type() == "string" {
            if params.len() != 1 {
                diagnostics.report_error(
                    format!(
                        "'char_at' expects exactly 1 argument (index), got {}",
                        params.len()
                    ),
                    Some(method.position),
                );
            }
            let mut idx_hir: Option<dream_hir::HExpr> = None;
            for param in params.iter() {
                let pt = self.analyze_expression(
                    param,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                )?;
                idx_hir = self.hir_take();
                if !pt.is_int() && !pt.is_unknown() {
                    diagnostics.report_error(
                        format!("'char_at' index must be int, got {}", pt.get_type()),
                        param.position(),
                    );
                }
            }
            self.hir_set_char_at(receiver.take(), idx_hir);
            return Ok(Some(Type::Char(synthetic_token(
                TokenKind::DataTypeToken,
                "char",
            ))));
        }

        // Object protocol: `x.to_string()` / `x.hash_code()` are available on every type. A
        // user-defined override (registered as `{Type}_to_string`) takes precedence and is resolved
        // by the normal method lookup below; otherwise fall back to the builtin protocol.
        if method.text == intrinsics::TO_STRING || method.text == intrinsics::HASH_CODE {
            let receiver_name = match Self::resolve_struct_parts(obj_type) {
                Some((base_name, generic_args)) => mangle_generic(&base_name, &generic_args),
                None => obj_type.get_type(),
            };
            let user_method = method_fn(&receiver_name, &method.text);
            let has_override = self.function_table.is_overloaded(&user_method)
                || self.function_table.get_function(&user_method).is_ok();
            if !has_override {
                if !params.is_empty() {
                    diagnostics.report_error(
                        format!("'{}' takes no arguments, got {}", method.text, params.len()),
                        Some(method.position),
                    );
                }
                if method.text == intrinsics::TO_STRING {
                    // A C-style enum's `to_string()` renders the variant name (e.g. `Color.Green`
                    // -> "Green") by mapping the discriminant to its interned name, rather than the
                    // generic object protocol (which would stringify the underlying integer).
                    if let Some(members) = self.enum_table.get(&receiver_name) {
                        let arms: Vec<(i64, String)> = members
                            .iter()
                            .map(|(name, value)| (*value as i64, name.clone()))
                            .collect();
                        self.hir_set_enum_name(receiver.take(), arms);
                        return Ok(Some(Type::String(synthetic_token(
                            TokenKind::DataTypeToken,
                            "string",
                        ))));
                    }
                    self.hir_set_to_string(receiver.take());
                    return Ok(Some(Type::String(synthetic_token(
                        TokenKind::DataTypeToken,
                        "string",
                    ))));
                }
                self.hir_set_hash_code(receiver.take());
                return Ok(Some(Type::Integer(synthetic_token(
                    TokenKind::DataTypeToken,
                    "int",
                ))));
            }
        }

        Ok(None)
    }
}
