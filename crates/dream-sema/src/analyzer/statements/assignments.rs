//! Indexed writes (`arr[i] = v`, including the class `set`-hook desugar) and member writes
//! (`obj.m = v`, including the static/instance setter desugar).

use super::*;
use dream_diagnostics::DiagnosticBag;
use crate::errors::SemanticError;
use crate::symbol_table::SymbolTable;
use dream_syntax::nodes::{ExpressionNode, FunctionNode, Type};
use dream_syntax::token::syntax_token::SyntaxToken;
use dream_syntax::token::token_kind::TokenKind;
use dream_types::method_fn;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(in crate::analyzer) fn analyze_index_assignment(
        &mut self,
        arr: &'a ExpressionNode<'a>,
        index: &'a ExpressionNode<'a>,
        right: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let array_type = self
            .analyze_expression(arr, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let array_hir = self.hir_take();

        // A `js`-typed receiver: `obj[key] = v` sets a JS property/element dynamically.
        if self.is_js_type(&array_type) {
            let _key_type = self
                .analyze_expression(index, parent_function, symbol_table, diagnostics)
                .unwrap_or(Type::Unknown);
            let key_hir = self.hir_take();
            let _value_type = self
                .analyze_expression(right, parent_function, symbol_table, diagnostics)
                .unwrap_or(Type::Unknown);
            let value_hir = self.hir_take();
            self.desugar_js_index_set(array_hir, key_hir, value_hir, index.position(), diagnostics);
            return Ok(());
        }

        // Class index-assignment: `obj[i] = v` on a struct receiver desugars to `obj.set(i, v)`
        // when an eligible `set` exists. Arrays keep the built-in path.
        if !matches!(array_type, Type::Array(_) | Type::Unknown)
            && Self::resolve_struct_parts(&array_type).is_some()
        {
            // The synthesized call re-evaluates the receiver, so drop the base HIR taken above.
            let _ = array_hir;
            return self.analyze_index_set(
                arr,
                index,
                right,
                &array_type,
                parent_function,
                symbol_table,
                diagnostics,
            );
        }

        let inner_type = match array_type {
            Type::Array(inner) => *inner,
            _ => {
                self.hir_fail();
                diagnostics.report_error(
                    format!("Cannot index into non-array type {}", array_type.get_type()),
                    arr.position(),
                );
                return Ok(());
            }
        };

        let index_type = self
            .analyze_expression(index, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let index_hir = self.hir_take();
        if !index_type.is_unknown() && !index_type.is_int() {
            diagnostics.report_error(
                format!(
                    "Array index must be of type int, got {}",
                    index_type.get_type()
                ),
                index.position(),
            );
        }

        let right_type = self
            .analyze_expression(right, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let value_hir = self.hir_take();
        self.compare_data_type(&inner_type, &right_type, &empty_span(), diagnostics)?;

        self.hir_assign_index(array_hir, index_hir, value_hir);
        Ok(())
    }

    /// Desugars a class index-assignment `obj[index] = value` to a call of the type's `@set`
    /// method when registered (accessible instance, non-async method taking two arguments; its
    /// return value is discarded).
    #[allow(clippy::too_many_arguments)]
    fn analyze_index_set(
        &mut self,
        arr: &'a ExpressionNode<'a>,
        index: &'a ExpressionNode<'a>,
        right: &ExpressionNode<'a>,
        obj_type: &Type,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        use crate::analyzer::declarations::protocol_hooks::ProtocolRole;
        let Some((hook, _)) = self.resolve_hook_or_diagnose(
            obj_type,
            ProtocolRole::Set,
            arr.position(),
            false,
            diagnostics,
            || {
                format!(
                    "type '{}' is not index-assignable (define '@set public fun ...(index, value)' to allow obj[index] = value)",
                    obj_type.get_type()
                )
            },
        ) else {
            return Ok(());
        };
        let set_tok = synthetic_token(TokenKind::IdentifierToken, &hook.surface_name);
        let call =
            ExpressionNode::MethodCall(arr, set_tok, None, vec![(*index).clone(), right.clone()]);
        let _ = self.analyze_expression(&call, parent_function, symbol_table, diagnostics)?;
        let call_hir = self.hir_take();
        self.hir_expr_stmt(call_hir);
        Ok(())
    }

    pub(in crate::analyzer) fn analyze_member_assignment(
        &mut self,
        obj: &'a ExpressionNode<'a>,
        member: &SyntaxToken,
        right: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        // Static property setter `Type.prop = v`: when the receiver names a type (not a local) and a
        // static setter exists, desugar to a static call `Type.set$prop(v)` (mirrors the instance
        // setter desugar below, but the receiver is the type rather than a value).
        if let ExpressionNode::Identifier(id) = obj {
            let is_local = symbol_table.borrow().get_symbol(id).is_ok();
            if !is_local {
                let setter = method_fn(&id.text, &setter_member_name(&member.text));
                if self.function_table.get_function(&setter).is_ok() {
                    let set_tok = synthetic_token(
                        TokenKind::IdentifierToken,
                        &setter_member_name(&member.text),
                    );
                    let call = ExpressionNode::MethodCall(obj, set_tok, None, vec![right.clone()]);
                    let _ =
                        self.analyze_expression(&call, parent_function, symbol_table, diagnostics)?;
                    let call_hir = self.hir_take();
                    self.hir_expr_stmt(call_hir);
                    return Ok(());
                }
            }
        }

        let obj_type = self
            .analyze_expression(obj, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let obj_hir = self.hir_take();

        // Tuple element write: `t.0 = v`.
        if let Type::Tuple(elems) = &obj_type {
            let Some(idx) = member.text.parse::<usize>().ok() else {
                diagnostics.report_error(
                    format!(
                        "tuple has no member '{}'; use .0, .1, …",
                        member.text
                    ),
                    Some(member.position),
                );
                self.hir_fail();
                return Ok(());
            };
            if idx >= elems.len() {
                diagnostics.report_error(
                    format!(
                        "tuple index {} is out of range for {}-element tuple",
                        idx,
                        elems.len()
                    ),
                    Some(member.position),
                );
                self.hir_fail();
                return Ok(());
            }
            let elem_ty = elems[idx].clone();
            let saved = self.current_expected_type.take();
            self.current_expected_type = Some(elem_ty.clone());
            let right_type = self
                .analyze_expression(right, parent_function, symbol_table, diagnostics)
                .unwrap_or(Type::Unknown);
            let value = self.hir_take();
            self.current_expected_type = saved;
            self.compare_data_type(&elem_ty, &right_type, &member.position, diagnostics)?;
            self.hir_assign_field(obj_hir, idx, value);
            return Ok(());
        }

        // A `js`-typed receiver: `obj.name = v` sets a JS property dynamically.
        if self.is_js_type(&obj_type) {
            let _value_type = self
                .analyze_expression(right, parent_function, symbol_table, diagnostics)
                .unwrap_or(Type::Unknown);
            let value_hir = self.hir_take();
            self.desugar_js_set(
                obj_hir,
                &member.text,
                value_hir,
                Some(member.position),
                diagnostics,
            );
            return Ok(());
        }

        match self.resolve_member_field(&obj_type, member, parent_function, diagnostics) {
            MemberField::Field {
                struct_name,
                field_type,
            } => {
                // Publish the field's declared type so the right-hand side can infer against it
                // (e.g. `this.items = []` resolves the empty literal to the field's element type).
                let saved_expected = self.current_expected_type.take();
                self.current_expected_type = Some(field_type.clone());
                let right_type = self
                    .analyze_expression(right, parent_function, symbol_table, diagnostics)
                    .unwrap_or(Type::Unknown);
                let value_hir = self.hir_take();
                self.current_expected_type = saved_expected;
                self.compare_data_type(&field_type, &right_type, &member.position, diagnostics)?;

                match self.struct_field_index(&struct_name, &member.text) {
                    Some(index) => self.hir_assign_field(obj_hir, index, value_hir),
                    None => self.hir_fail(),
                }
                Ok(())
            }
            MemberField::NotAStruct => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "Cannot access member of non-class type {}",
                        obj_type.get_type()
                    ),
                    Some(member.position),
                );
                Ok(())
            }
            MemberField::StructNotFound { struct_name } => {
                self.hir_fail();
                diagnostics.report_error(
                    format!("Struct '{}' not found", struct_name),
                    Some(member.position),
                );
                Ok(())
            }
            MemberField::NotAField { struct_name } => {
                // Not a field: `obj.prop = v` may write a property setter, which desugars to a call
                // of the (internally named) setter method. The call carries its own privacy/type
                // check, and its (discarded) result becomes the assignment statement.
                let setter = method_fn(&struct_name, &setter_member_name(&member.text));
                if self.function_table.get_function(&setter).is_ok() {
                    let set_tok = synthetic_token(
                        TokenKind::IdentifierToken,
                        &setter_member_name(&member.text),
                    );
                    let call = ExpressionNode::MethodCall(obj, set_tok, None, vec![right.clone()]);
                    let _ =
                        self.analyze_expression(&call, parent_function, symbol_table, diagnostics)?;
                    let call_hir = self.hir_take();
                    self.hir_expr_stmt(call_hir);
                    Ok(())
                } else {
                    self.hir_fail();
                    diagnostics.report_error(
                        format!(
                            "Field '{}' not found in class '{}'",
                            member.text, struct_name
                        ),
                        Some(member.position),
                    );
                    Ok(())
                }
            }
        }
    }
}
