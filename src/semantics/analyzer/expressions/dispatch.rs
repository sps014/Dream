//! The `analyze_expression` dispatch match and the class-indexer read desugar it delegates to.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::strip_nullable;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(in crate::semantics::analyzer) fn analyze_expression(
        &mut self,
        expression: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        match expression {
            ExpressionNode::Literal(number) => {
                self.hir_set_literal(number);
                Ok(number.clone())
            }
            ExpressionNode::ArrayLiteral(elements) => {
                // The element type expected for this literal, taken from the surrounding array-typed
                // context (`let xs: int[] = ...`, `return ...`, an argument slot, a field, etc.). It
                // is threaded down into each element so nested empty literals (`int[][] = [[]]`) and
                // empty elements infer their element type instead of falling through as untyped.
                let expected_elem = match &self.current_expected_type {
                    Some(Type::Array(elem)) => Some((**elem).clone()),
                    _ => None,
                };

                if elements.is_empty() {
                    // With an array-typed context the empty literal takes that element type; without
                    // one it is genuinely ambiguous (nothing to infer from), so reject it clearly.
                    if let Some(elem) = expected_elem {
                        self.hir_set_empty_array(&elem);
                        return Ok(Type::Array(Box::new(elem)));
                    }
                    self.hir_none();
                    self.hir_fail();
                    diagnostics.report_error(
                        "cannot infer the element type of an empty array literal; add an array type annotation, e.g. `let xs: int[] = [];`".to_string(),
                        expression.position(),
                    );
                    return Ok(Type::Array(Box::new(Type::Void)));
                }

                let saved_expected = self.current_expected_type.take();
                self.current_expected_type = expected_elem;
                let first_type = self.analyze_expression(
                    &elements[0],
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                let mut elem_hirs = vec![self.hir_take()];

                for elem in elements.iter().skip(1) {
                    let element_type =
                        self.analyze_expression(elem, parent_function, symbol_table, diagnostics)?;
                    elem_hirs.push(self.hir_take());
                    self.compare_data_type(&first_type, &element_type, &empty_span(), diagnostics)?;
                }
                self.current_expected_type = saved_expected;

                let array_type = Type::Array(Box::new(first_type));
                self.hir_set_array_lit(elem_hirs, &array_type);
                Ok(array_type)
            }
            ExpressionNode::IndexAccess(array_expr, index_expr) => {
                let array_type = self.analyze_expression(
                    array_expr,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                let array_hir = self.hir_take();

                // A `js`-typed receiver indexes dynamically (`obj[key]`), with a string or numeric
                // key. Must precede the class/string indexer desugar, which would look for a `get`.
                if self.is_js_type(&array_type) {
                    let key_type = self.analyze_expression(
                        index_expr,
                        parent_function,
                        symbol_table,
                        diagnostics,
                    )?;
                    let key_hir = self.hir_take();
                    let _ = key_type;
                    self.desugar_js_index_get(
                        array_hir,
                        key_hir,
                        index_expr.position(),
                        diagnostics,
                    );
                    return Ok(Self::js_type());
                }

                // Class/string indexer: `obj[i]` on a struct or `string` receiver desugars to
                // `obj.get(i)` when an eligible `get` exists (`string` exposes one via `extend
                // string`, yielding a `char`). Arrays keep the built-in index path; `Unknown` is a
                // poison carried from an earlier error and must not cascade.
                if !matches!(array_type, Type::Array(_) | Type::Unknown)
                    && (Self::resolve_struct_parts(&array_type).is_some()
                        || matches!(array_type, Type::String(_)))
                {
                    // The synthesized call re-evaluates the receiver, so drop the base HIR taken above.
                    let _ = array_hir;
                    return self.analyze_index_get(
                        array_expr,
                        index_expr,
                        &array_type,
                        parent_function,
                        symbol_table,
                        diagnostics,
                    );
                }

                let inner_type = match array_type {
                    Type::Array(inner) => *inner,
                    // Don't cascade if the base was already poisoned by an earlier error.
                    Type::Unknown => Type::Unknown,
                    _ => {
                        diagnostics.report_error(
                            format!("Cannot index into non-array type {}", array_type.get_type()),
                            array_expr.position(),
                        );
                        Type::Unknown
                    }
                };

                let index_type = self.analyze_expression(
                    index_expr,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                let index_hir = self.hir_take();
                if !index_type.is_unknown() && !index_type.is_int() {
                    diagnostics.report_error(
                        format!(
                            "Array index must be of type int, got {}",
                            index_type.get_type()
                        ),
                        index_expr.position(),
                    );
                }

                self.hir_set_index(array_hir, index_hir, &inner_type);
                Ok(inner_type)
            }
            ExpressionNode::Unary(opr, right) => {
                let right_type =
                    self.analyze_expression(right, parent_function, symbol_table, diagnostics)?;
                let operand = self.hir_take();
                match opr.kind {
                    TokenKind::BangToken => {
                        if !right_type.is_unknown() && !right_type.is_bool() {
                            diagnostics.report_error(
                                format!("! operator requires bool, got {}", right_type.get_type()),
                                Some(opr.position),
                            );
                        }
                        let result = Type::Boolean(opr.clone());
                        self.hir_set_unary(opr, operand, &result);
                        Ok(result)
                    }
                    TokenKind::PlusToken | TokenKind::MinusToken => {
                        if !right_type.is_unknown()
                            && !matches!(
                                right_type,
                                Type::Integer(_)
                                    | Type::Long(_)
                                    | Type::UInt(_)
                                    | Type::ULong(_)
                                    | Type::Byte(_)
                                    | Type::Float(_)
                                    | Type::Double(_)
                            )
                        {
                            diagnostics.report_error(
                                format!(
                                    "unary +/- requires a numeric type, got {}",
                                    right_type.get_type()
                                ),
                                Some(opr.position),
                            );
                        }
                        self.hir_set_unary(opr, operand, &right_type);
                        Ok(right_type)
                    }
                    _ => {
                        diagnostics.report_error(
                            format!("unknown unary operator {}", opr.text),
                            Some(opr.position),
                        );
                        self.hir_none();
                        Ok(right_type)
                    }
                }
            }
            ExpressionNode::Binary(left, opr, right) => Ok(self.analyze_binary_expression(
                left,
                opr,
                right,
                parent_function,
                symbol_table,
                diagnostics,
            )?),
            ExpressionNode::Identifier(id) => {
                Ok(self.analyze_identifier(id, symbol_table, diagnostics)?)
            }
            ExpressionNode::FunctionCall(name, generic_args, params) => {
                // `analyze_function_call` records the call's HIR itself (only for a resolvable,
                // non-generic, non-overloaded, non-async free function; otherwise it clears `last`).
                let t = self.analyze_function_call(
                    name,
                    generic_args,
                    params,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::IsExpression(left, right_type, _binding) => {
                // `is` always evaluates to a bool. A concrete static operand folds to a compile-time
                // result; an `object` or interface-typed operand emits a runtime `$object_tag`
                // comparison. (The optional `_binding` is handled by the statement layer — `if`/
                // `while` conditions and top-level `&&` chains, see `statements.rs` — which flow-types
                // it into the guarded branch/body; the expression itself ignores the binding here.)
                let left_type =
                    self.analyze_expression(left, parent_function, symbol_table, diagnostics)?;
                let left_hir = self.hir_take();
                let left_name = left_type.get_type();
                let right_name = right_type.get_type();
                let stripped = strip_nullable(&left_name);
                if left_type.is_unknown() {
                    self.hir_none();
                } else if stripped == "object" || self.is_interface_name(stripped) {
                    self.hir_set_is_type(left_hir, right_type);
                } else {
                    self.hir_set_bool(stripped == strip_nullable(&right_name));
                }
                Ok(Type::Boolean(synthetic_token(
                    TokenKind::BooleanToken,
                    "true",
                )))
            }
            ExpressionNode::Parenthesized(expr) => {
                Ok(self.analyze_expression(expr, parent_function, symbol_table, diagnostics)?)
            }
            ExpressionNode::Ternary(condition, then_expr, else_expr) => {
                let cond_type =
                    self.analyze_expression(condition, parent_function, symbol_table, diagnostics)?;
                let cond_hir = self.hir_take();
                if !cond_type.is_bool() {
                    diagnostics.report_error(
                        format!(
                            "Ternary condition must be of type bool, got {}",
                            cond_type.get_type()
                        ),
                        condition.position(),
                    );
                }
                let then_type =
                    self.analyze_expression(then_expr, parent_function, symbol_table, diagnostics)?;
                let then_hir = self.hir_take();
                let else_type =
                    self.analyze_expression(else_expr, parent_function, symbol_table, diagnostics)?;
                let else_hir = self.hir_take();
                // Both branches must agree; reuse the standard compatibility check.
                self.compare_data_type(&then_type, &else_type, &empty_span(), diagnostics)?;
                self.hir_set_ternary(cond_hir, then_hir, else_hir, &then_type);
                Ok(then_type)
            }
            ExpressionNode::Switch(subject, arms) => {
                // `analyze_pattern_switch` desugars the value-position switch and records its result temp read.
                let t = self.analyze_pattern_switch(
                    subject,
                    arms,
                    parent_function,
                    symbol_table,
                    true,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::MemberAccess(obj, member) => {
                // `analyze_member_access` records the HIR itself (struct-field read / enum value).
                let t = self.analyze_member_access(
                    obj,
                    member,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::Cast(target_type, expr) => {
                // `analyze_cast` records the cast's HIR itself.
                let t = self.analyze_cast(
                    target_type,
                    expr,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::MethodCall(obj, method, generic_args, params) => {
                let ctx = super::super::AnalyzerContext {
                    parent_function,
                    symbol_table,
                };
                let t =
                    self.analyze_method_call(obj, method, generic_args, params, &ctx, diagnostics)?;
                // `analyze_method_call` records the `MethodCall`/`Call` (or clears `last`) itself.
                Ok(t)
            }
            ExpressionNode::Await(inner) => {
                let fut =
                    self.analyze_expression(inner, parent_function, symbol_table, diagnostics)?;
                let inner_hir = self.hir_take();
                if fut.is_unknown() {
                    self.hir_none();
                    return Ok(Type::Unknown);
                }
                // Awaiting a dynamic `js` value treats it as a JS Promise: desugar to
                // `await js.__await(inner)`, whose async bridge yields `Future<js>` and resolves to
                // the awaited value as another `js`.
                if self.is_js_type(&fut) {
                    let fut_hir = self.desugar_js_await(inner_hir);
                    let opt = Self::option_js_type();
                    self.hir_set_await(fut_hir, &opt);
                    return Ok(opt);
                }
                match Self::future_inner_type(&fut) {
                    Some(t) => {
                        self.hir_set_await(inner_hir, &t);
                        Ok(t)
                    }
                    None => {
                        self.hir_none();
                        Err(report(
                            diagnostics,
                            format!("'await' expects a Future value, got {}", fut.get_type()),
                            inner.position(),
                        ))
                    }
                }
            }
        }
    }

    /// Desugars a class indexer read `obj[index]` to `obj.get(index)` when `obj_type` exposes an
    /// eligible `get` (see [`Analyzer::resolve_hook_method`]): an accessible instance, non-async
    /// method taking one argument and returning a (non-`void`) value. Any other same-named `get`
    /// (static/async/void/wrong arity) is left as an ordinary method and this site reports why the
    /// value cannot be indexed, rather than silently rewriting the call.
    fn analyze_index_get(
        &mut self,
        array_expr: &'a ExpressionNode<'a>,
        index_expr: &'a ExpressionNode<'a>,
        obj_type: &Type,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let info = match self.resolve_hook_or_diagnose(
            obj_type,
            "get",
            1,
            array_expr.position(),
            true,
            diagnostics,
            |reason| format!("type '{}' cannot be indexed: {}", obj_type.get_type(), reason),
            || {
                format!(
                    "type '{}' has no indexer (define 'public fun get(index): T' to allow obj[index])",
                    obj_type.get_type()
                )
            },
        ) {
            Some(info) => info,
            None => return Ok(Type::Unknown),
        };
        if matches!(info.return_type, None | Some(Type::Void)) {
            self.hir_fail();
            self.hir_none();
            diagnostics.report_error(
                format!(
                    "type '{}' has no indexer: its 'get' must return a value",
                    obj_type.get_type()
                ),
                array_expr.position(),
            );
            return Ok(Type::Unknown);
        }
        let get_tok = synthetic_token(TokenKind::IdentifierToken, "get");
        let call =
            ExpressionNode::MethodCall(array_expr, get_tok, None, vec![(*index_expr).clone()]);
        self.analyze_expression(&call, parent_function, symbol_table, diagnostics)
    }
}
