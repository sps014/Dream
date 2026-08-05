//! `expr?` (Result/Option try-propagation). [`Analyzer::analyze_try_expression`] desugars a
//! postfix `?` into a two-arm `Switch` over the operand's `Result<T, E>`/`Option<T>`: the
//! success arm binds the payload as the whole expression's value, and the failure arm
//! re-constructs the failure/absence variant as the enclosing function's declared return type and
//! `return`s it immediately. No surface-syntax desugar is possible (Dream has no block
//! expressions), so this builds the `Switch`/`Return` HIR directly, mirroring the machinery
//! [`super::lowering`] uses for a real pattern-matching `switch`.

use super::*;
use dream_diagnostics::DiagnosticBag;
use dream_hir::{Binding, HExpr, HExprKind, LocalId};
use crate::errors::SemanticError;
use crate::symbol_table::SymbolTable;
use dream_syntax::nodes::{ExpressionNode, FunctionNode, Type};
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(in crate::analyzer) fn analyze_try_expression(
        &mut self,
        inner: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let position = inner.position();

        let operand_type =
            self.analyze_expression(inner, parent_function, symbol_table, diagnostics)?;
        let operand_hir = self.hir_take();

        let wrapper_shape = Self::resolve_struct_parts(&operand_type).and_then(|(base, args)| {
            match base.as_str() {
                "Result" if args.len() == 2 => Some((base, args)),
                "Option" if args.len() == 1 => Some((base, args)),
                _ => None,
            }
        });
        let Some((op_base, op_args)) = wrapper_shape else {
            self.hir_fail();
            self.hir_none();
            return Err(report(
                diagnostics,
                format!(
                    "'?' requires a Result<T, E> or Option<T> operand, got {}",
                    operand_type.display_name()
                ),
                position,
            ));
        };

        let return_type = parent_function.return_type.clone();
        let ret_shape = return_type.as_ref().and_then(|rt| {
            Self::resolve_struct_parts(rt).filter(|(base, args)| {
                base == &op_base && ((op_base == "Result" && args.len() == 2) || args.len() == 1)
            })
        });
        let (Some(return_type), Some((_ret_base, ret_args))) = (return_type, ret_shape) else {
            self.hir_fail();
            self.hir_none();
            let found = parent_function
                .return_type
                .as_ref()
                .map(|t| t.display_name())
                .unwrap_or_else(|| "void".to_string());
            return Err(report(
                diagnostics,
                format!(
                    "'?' on a {} requires the enclosing function to return a matching {}<...>, got {}",
                    op_base, op_base, found
                ),
                position,
            ));
        };

        // The error type must match exactly between the operand and the function's return type
        // (`Result<T, E>` propagates its `E` unchanged; only the success payload type may differ).
        if op_base == "Result" {
            self.compare_data_type(&op_args[1], &ret_args[1], &empty_span(), diagnostics)?;
        }

        let span = position.unwrap_or_else(empty_span);
        self.ensure_union_instantiated(&op_base, &op_args, &span, diagnostics);
        self.ensure_union_instantiated(&op_base, &ret_args, &span, diagnostics);

        let (ok_name, err_name) = if op_base == "Result" {
            ("Ok", "Err")
        } else {
            ("Some", "None")
        };

        let op_mangled = operand_type.get_type();
        let ret_mangled = return_type.get_type();
        let op_info = self.union_table.get(&op_mangled).cloned();
        let op_def = self
            .type_ctx
            .defs
            .lookup(dream_types::DefKind::Union, &op_mangled);
        let ret_def = self
            .type_ctx
            .defs
            .lookup(dream_types::DefKind::Union, &ret_mangled);

        let success_ty = op_args[0].clone();

        let (Some(op_info), Some(op_def), Some(ret_def)) = (op_info, op_def, ret_def) else {
            self.hir_fail();
            self.hir_none();
            return Ok(success_ty);
        };
        let (Some(ok_variant), Some(err_variant)) =
            (op_info.variant(ok_name), op_info.variant(err_name))
        else {
            self.hir_fail();
            self.hir_none();
            return Ok(success_ty);
        };
        let ok_disc = ok_variant.discriminant as usize;
        let err_disc = err_variant.discriminant as usize;
        let err_has_payload = !err_variant.fields.is_empty();

        let success_local = self.hir_alloc_local("__try_ok", &success_ty);
        let err_payload_ty = ret_args.get(1).cloned();
        let err_local = if err_has_payload {
            err_payload_ty
                .as_ref()
                .and_then(|t| self.hir_alloc_local("__try_err", t))
        } else {
            None
        };

        let mut ok = operand_hir.is_some() && success_local.is_some();

        let ok_arm = self.hir_variant_arm(
            op_def,
            ok_disc,
            vec![success_local.unwrap_or(LocalId(0))],
            vec![],
        );

        self.hir_open_block();
        let err_value = if err_has_payload {
            match (err_local, &err_payload_ty) {
                (Some(local), Some(ty)) => {
                    let ty_id = self.type_ctx.lower(ty);
                    let read = HExpr::new(ty_id, HExprKind::Var(Binding::Local(local)));
                    self.hir_set_union_new(ret_def, err_disc, vec![Some(read)], &return_type);
                    self.hir_take()
                }
                _ => {
                    ok = false;
                    None
                }
            }
        } else {
            self.hir_set_union_new(ret_def, err_disc, vec![], &return_type);
            self.hir_take()
        };
        match err_value {
            Some(v) => self.hir_return_value(Some(v), None),
            None => ok = false,
        }
        let err_body = self.hir_close_block();
        let err_bindings = match err_local {
            Some(l) => vec![l],
            None => vec![],
        };
        let err_arm = self.hir_variant_arm(op_def, err_disc, err_bindings, err_body);

        self.hir_switch(operand_hir, vec![ok_arm, err_arm], vec![], ok);

        if ok {
            let ty_id = self.type_ctx.lower(&success_ty);
            self.hir_set_local_read(success_local.unwrap_or(LocalId(0)), ty_id);
        } else {
            self.hir_fail();
            self.hir_none();
        }

        Ok(success_ty)
    }
}
