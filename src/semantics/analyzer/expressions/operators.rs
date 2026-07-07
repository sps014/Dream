//! Binary-operator typing: null-coalescing, string concatenation, user-defined `equals` dispatch,
//! comparisons, and arithmetic.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::types::method_fn;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(super) fn analyze_binary_expression(
        &mut self,
        left: &ExpressionNode<'a>,
        opr: &SyntaxToken,
        right: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let left_value =
            self.analyze_expression(left, parent_function, symbol_table, diagnostics)?;
        let left_hir = self.hir_take();
        let right_value =
            self.analyze_expression(right, parent_function, symbol_table, diagnostics)?;
        let right_hir = self.hir_take();

        // Null-coalescing `a ?? b`: `a` should be nullable; the result is the unwrapped element
        // type, and `b` must be assignable to it (or itself nullable of the same element type).
        if opr.kind == TokenKind::QuestionQuestionToken {
            let result_type = match &left_value {
                Type::Nullable(inner) => (**inner).clone(),
                other => other.clone(),
            };
            let right_unwrapped = match &right_value {
                Type::Nullable(inner) => (**inner).clone(),
                other => other.clone(),
            };
            self.compare_data_type(&result_type, &right_unwrapped, &opr.position, diagnostics)?;
            self.hir_set_coalesce(left_hir, right_hir, &result_type);
            return Ok(result_type);
        }

        // String concatenation: `string + T` (or `T + string`) yields a string, auto-converting
        // the non-string operand through its `to_string` (the object protocol, or a C-style enum's
        // variant-name rendering) in codegen. This means `"count = " + n` works for any `n` with no
        // explicit `.to_string()`.
        if opr.kind == TokenKind::PlusToken {
            let left_is_string = left_value.is_string();
            let right_is_string = right_value.is_string();
            if left_is_string || right_is_string {
                self.hir_set_concat(left_hir, &left_value, right_hir, &right_value);
                return Ok(if left_is_string {
                    left_value
                } else {
                    right_value
                });
            }
        }

        self.compare_data_type(&left_value, &right_value, &opr.position, diagnostics)?;
        match (&left_value, &opr.kind) {
            (Type::String(_), TokenKind::PlusToken) => {}
            // Reference (identity) equality is allowed on strings and objects.
            (Type::String(_), TokenKind::EqualEqualToken)
            | (Type::String(_), TokenKind::NotEqualToken) => {}
            (Type::String(_), _) => {
                diagnostics.report_error(
                    format!("Cannot perform operation {} on string", opr.text),
                    Some(opr.position),
                );
            }
            (_, _) => {}
        };

        // User-defined value equality: for `==`/`!=` where the operand's static type is a user type
        // that implements `Equatable<Self>`, dispatch to its `equals` method (a static call),
        // negating the result for `!=`. Primitives, strings, and null comparisons keep the built-in
        // behavior handled above/below.
        if matches!(
            opr.kind,
            TokenKind::EqualEqualToken | TokenKind::NotEqualToken
        ) {
            if let Some(equals_fn) = self.equatable_equals_fn(&left_value) {
                let bool_ty = Type::Boolean(opr.clone());
                self.hir_set_method_call(left_hir, &equals_fn, vec![right_hir], &bool_ty);
                if opr.kind == TokenKind::NotEqualToken {
                    self.hir_negate_last();
                }
                return Ok(bool_ty);
            }
        }

        let is_bool_result = matches!(
            opr.kind,
            TokenKind::EqualEqualToken
                | TokenKind::NotEqualToken
                | TokenKind::GreaterThanToken
                | TokenKind::GreaterThanEqualToken
                | TokenKind::SmallerThanToken
                | TokenKind::SmallerThanEqualToken
                | TokenKind::AmpersandAmpersandToken
                | TokenKind::PipePipeToken
        );
        let result_type = if is_bool_result {
            Type::Boolean(opr.clone())
        } else {
            left_value.clone()
        };
        self.hir_set_binary(left_hir, opr, right_hir, &result_type);
        Ok(result_type)
    }

    /// If `==`/`!=` on a value of type `left` should dispatch to a user-defined `equals`, returns
    /// the mangled method symbol (e.g. `Money_equals`). Applies when `left`'s concrete (non-nullable)
    /// type is a class/struct that implements `Equatable<Self>`; the caller has already verified the
    /// operands are type-compatible.
    fn equatable_equals_fn(&self, left: &Type) -> Option<String> {
        let (base, args) = Self::resolve_struct_parts(left)?;
        let recv = mangle_generic(&base, &args);
        // The interface argument is the receiver type itself (the `Equatable<Self>` convention),
        // mangled exactly as `validate_implements` recorded it.
        let self_ty = match left {
            Type::Nullable(inner) => (**inner).clone(),
            other => other.clone(),
        };
        let iface = mangle_generic("Equatable", std::slice::from_ref(&self_ty));
        if self.class_implements(&recv, &iface) {
            return Some(method_fn(&recv, "equals"));
        }
        None
    }
}
