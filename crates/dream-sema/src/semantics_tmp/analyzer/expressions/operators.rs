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

/// True for the binary bitwise operator tokens (`&`/`|`/`^`/`<<`/`>>`) — the ones restricted to
/// integer operands. `&&`/`||` are logical (bool-only, checked elsewhere), not bitwise.
fn is_bitwise_op(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::BitWiseAmpersandToken
            | TokenKind::BitWisePipeToken
            | TokenKind::BitWiseXorToken
            | TokenKind::ShiftLeftToken
            | TokenKind::ShiftRightToken
    )
}

impl<'a> Analyzer<'a> {
    pub(super) fn analyze_binary_expression(
        &mut self,
        left: &'a ExpressionNode<'a>,
        opr: &SyntaxToken,
        right: &'a ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let left_value =
            self.analyze_expression(left, parent_function, symbol_table, diagnostics)?;
        let left_hir = self.hir_take();

        // `x is T t && t.ok()`: every `is`-binding guaranteed by `left` (a bare `is`, or reachable
        // through a top-level `&&` chain within `left` itself) is visible while analyzing `right`,
        // since short-circuiting means `right` only ever runs once `left` is true. The alias is
        // popped again immediately after, so it never leaks past this one conjunct.
        let alias_mark = self.is_binding_aliases.len();
        if opr.kind == TokenKind::AmpersandAmpersandToken {
            let mut bindings = Vec::new();
            Self::collect_is_bindings(left, &mut bindings);
            for (name, ty, operand) in bindings {
                self.is_binding_aliases
                    .push((name.text.clone(), ty.clone(), operand));
            }
        }
        let right_value =
            self.analyze_expression(right, parent_function, symbol_table, diagnostics)?;
        let right_hir = self.hir_take();
        self.is_binding_aliases.truncate(alias_mark);

        // `a ?? b`: pure sugar for `a.unwrap_or(b)` on an `Option<T>` left operand — the same
        // method the stdlib already exposes, just spelled as an operator for the common inline
        // "unwrap with a default" case. `a` must be `Option<T>`; `b` must be assignable to `T`.
        if opr.kind == TokenKind::QuestionQuestionToken {
            let Some((base, args)) = Self::resolve_struct_parts(&left_value) else {
                diagnostics.report_error(
                    format!(
                        "'??' requires an Option<T> operand, got {}",
                        left_value.display_name()
                    ),
                    Some(opr.position),
                );
                return Ok(right_value);
            };
            if base != "Option" || args.len() != 1 {
                diagnostics.report_error(
                    format!(
                        "'??' requires an Option<T> operand, got {}",
                        left_value.display_name()
                    ),
                    Some(opr.position),
                );
                return Ok(right_value);
            }
            self.ensure_union_instantiated("Option", &args, &opr.position, diagnostics);
            let inner = args[0].clone();
            self.compare_data_type(&inner, &right_value, &opr.position, diagnostics)?;
            let recv = mangle_generic(&base, &args);
            let method = method_fn(&recv, "unwrap_or");
            self.hir_set_method_call(left_hir, &method, vec![right_hir], &inner);
            return Ok(inner);
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

        // User-defined operator overload: `@operator("+")`/`@operator("==")`/etc. on the left
        // operand's type. Checked before the built-in numeric/string rules below so a struct's
        // overload always wins over (what would otherwise be) a type error. `!=` is handled
        // separately below (as the negation of a registered `@operator("==")`), since there is no
        // standalone `!=` symbol to register.
        if let Some(op_method) = self.operator_binary_fn(&left_value, opr.kind) {
            let param_type = op_method.param_type;
            let return_type = op_method.return_type;
            let mangled_name = op_method.mangled_name;
            if let Some(param_type) = &param_type {
                self.compare_data_type(param_type, &right_value, &opr.position, diagnostics)?;
            }
            self.hir_set_method_call(left_hir, &mangled_name, vec![right_hir], &return_type);
            return Ok(return_type);
        }

        // User-defined ordering: `@operator`-free structs implementing `Comparable<Self>` get
        // `<`/`<=`/`>`/`>=` for free, lowered to `a.compare(b) <op> 0`.
        if matches!(
            opr.kind,
            TokenKind::GreaterThanToken
                | TokenKind::GreaterThanEqualToken
                | TokenKind::SmallerThanToken
                | TokenKind::SmallerThanEqualToken
        ) {
            if let Some(compare_fn) = self.comparable_compare_fn(&left_value) {
                self.compare_data_type(&left_value, &right_value, &opr.position, diagnostics)?;
                let bool_ty = Type::Boolean(opr.clone());
                let int_ty = Type::Integer(opr.clone());
                self.hir_set_method_call(left_hir, &compare_fn, vec![right_hir], &int_ty);
                self.hir_compare_last_to_zero(opr.kind);
                return Ok(bool_ty);
            }
        }

        self.compare_data_type(&left_value, &right_value, &opr.position, diagnostics)?;

        // Bitwise ops (`&`/`|`/`^`/`<<`/`>>`) are only meaningful on integer operands
        // (`int`/`long`/`uint`/`ulong`/`byte`); `float`/`double` have no well-defined bitwise
        // lowering. Caught here rather than left to the backend, which would otherwise emit an
        // invalid WASM instruction (e.g. a nonexistent `f64.and`) instead of a clean diagnostic.
        if is_bitwise_op(opr.kind) && !left_value.is_unknown() && !left_value.is_integer() {
            diagnostics.report_error(
                format!(
                    "'{}' requires an integer operand (int/long/uint/ulong/byte), got {}",
                    opr.text,
                    left_value.display_name()
                ),
                Some(opr.position),
            );
            return Ok(Type::Unknown);
        }

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
            // `!=` has no standalone registered symbol; a `@operator("==")` overload also powers it
            // (negated). `==` itself is already handled by the generic operator-overload dispatch
            // above, since `EqualEqualToken` maps directly to `OperatorSymbol::Eq`.
            if opr.kind == TokenKind::NotEqualToken {
                if let Some(op_method) =
                    self.operator_binary_fn(&left_value, TokenKind::EqualEqualToken)
                {
                    if let Some(param_type) = &op_method.param_type {
                        self.compare_data_type(
                            param_type,
                            &right_value,
                            &opr.position,
                            diagnostics,
                        )?;
                    }
                    let bool_ty = Type::Boolean(opr.clone());
                    self.hir_set_method_call(
                        left_hir,
                        &op_method.mangled_name,
                        vec![right_hir],
                        &bool_ty,
                    );
                    self.hir_negate_last();
                    return Ok(bool_ty);
                }
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
    /// the mangled method symbol (e.g. `Money_equals`). Applies when `left`'s concrete type is a
    /// class/struct that implements `Equatable<Self>`; the caller has already verified the operands
    /// are type-compatible.
    fn equatable_equals_fn(&self, left: &Type) -> Option<String> {
        let (base, args) = Self::resolve_struct_parts(left)?;
        let recv = mangle_generic(&base, &args);
        // The interface argument is the receiver type itself (the `Equatable<Self>` convention),
        // mangled exactly as `validate_implements` recorded it.
        let iface = mangle_generic("Equatable", std::slice::from_ref(left));
        if self.class_implements(&recv, &iface) {
            return Some(method_fn(&recv, "equals"));
        }
        None
    }

    /// If `<`/`<=`/`>`/`>=` on a value of type `left` should dispatch to a user-defined `compare`,
    /// returns the mangled method symbol (e.g. `Money_compare`). Applies when `left`'s concrete type
    /// implements `Comparable<Self>` (and has no more specific `@operator`-tagged overload, checked
    /// by the caller first).
    fn comparable_compare_fn(&self, left: &Type) -> Option<String> {
        let (base, args) = Self::resolve_struct_parts(left)?;
        let recv = mangle_generic(&base, &args);
        let iface = mangle_generic("Comparable", std::slice::from_ref(left));
        if self.class_implements(&recv, &iface) {
            return Some(method_fn(&recv, "compare"));
        }
        None
    }
}
