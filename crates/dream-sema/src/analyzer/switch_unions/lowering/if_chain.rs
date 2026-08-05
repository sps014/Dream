//! The pattern-`switch` lowering paths, plus the subject-resolution/arm-result helpers they share:
//! - [`Analyzer::analyze_pattern_switch`]: the `Switch`/br_table fast path for flat, unguarded
//!   variant/const/catch-all arms (including or-patterns and small literal ranges expanded into
//!   multi-key arms).
//! - [`Analyzer::analyze_pattern_switch_hybrid`]: outer `Switch` plus residual if-chain inside each
//!   arm for nested/literal sub-patterns and guards.
//! - [`Analyzer::analyze_pattern_switch_chain`]: full if-chain fallback when no outer Switch key
//!   exists (unexpanded ranges; see [`Analyzer::pattern_switch_needs_full_chain`]).

use super::super::*;
use dream_diagnostics::DiagnosticBag;
use crate::errors::SemanticError;
use crate::symbol_table::SymbolTable;
use dream_syntax::nodes::{
    ExpressionNode, FunctionNode, SwitchArm, Type,
};
use dream_syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// General pattern-`switch` lowering (guards + nested/literal sub-patterns) as a flag-gated
    /// if-chain: evaluates the subject once, then for each arm emits `if (!done && <tests>) {
    /// <binds>; [if (<guard>)] { done = true; <body> } }`. A failed guard leaves `done` false so the
    /// next arm is tried. Type-checking (pattern checks, guard/body analysis, exhaustiveness) mirrors
    /// the `Switch` path.
    pub(crate) fn analyze_pattern_switch_chain(
        &mut self,
        subject: &ExpressionNode<'a>,
        arms: &[SwitchArm<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        use dream_hir::{BinOp, HStmt};

        let (subject_type, subject_hir, subject_base, union_info) =
            self.resolve_switch_subject(subject, parent_function, symbol_table, diagnostics)?;

        let bool_type = Type::Boolean(synthetic_token(TokenKind::BooleanToken, "bool"));
        let bool_ty = self.type_ctx.interner.bool();
        let subj_ty_id = self.type_ctx.lower(&subject_type);

        let mut emit_ok = subject_hir.is_some();

        // Bind the subject once and initialize the `done` flag.
        let subj_local = self.hir_alloc_local("__switch_subj", &subject_type);
        match (subj_local, subject_hir) {
            (Some(local), Some(sh)) => self.hir_push_stmt(HStmt::Let {
                local,
                ty: subj_ty_id,
                value: sh,
            }),
            _ => emit_ok = false,
        }
        let done_local = self.hir_alloc_local("__switch_done", &bool_type);
        if let Some(done) = done_local {
            let init = self.hx_bool(false);
            self.hir_push_stmt(HStmt::Let {
                local: done,
                ty: bool_ty,
                value: init,
            });
        }

        let subj_read =
            |s: &Self| s.hx_local(subj_local.unwrap_or(dream_hir::LocalId(0)), subj_ty_id);

        let mut arm_value_type: Option<Type> = None;
        let mut catch_all_index: Option<usize> = None;
        let mut result_temp: Option<dream_hir::LocalId> = None;
        let mut result_ty_id: Option<dream_types::TypeId> = None;

        for (i, arm) in arms.iter().enumerate() {
            if catch_all_index.is_some() {
                diagnostics.report_error(
                    "Unreachable switch arm: a previous arm already matches everything".to_string(),
                    arm.pattern.position(),
                );
            }

            let arm_scope = Rc::new(RefCell::new(SymbolTable::new(Some(symbol_table.clone()))));
            (*symbol_table).borrow_mut().add_child(arm_scope.clone());

            let info = self.check_pattern(&arm.pattern, &subject_type, &arm_scope, diagnostics)?;

            // Build the arm's tests + payload bindings from the resolved subject value.
            let sr = subj_read(self);
            let (conds, binds) = match self.compile_pattern(&sr, &subject_type, &arm.pattern) {
                Some(cb) => cb,
                None => {
                    emit_ok = false;
                    (vec![], vec![])
                }
            };

            // then-branch: declare bindings, then (optionally guard and) run the body.
            self.hir_open_block();
            for (name, ty, expr) in binds {
                self.hir_declare_local(&name, &ty, Some(expr));
            }

            let mut run_body =
                |s: &mut Self, diags: &mut DiagnosticBag| -> Result<(), SemanticError> {
                    if let Some(done) = done_local {
                        let t = s.hx_bool(true);
                        s.hir_assign_local_id(done, Some(t));
                    }
                    s.analyze_switch_arm_result(
                        arm,
                        parent_function,
                        &arm_scope,
                        is_expression,
                        &mut arm_value_type,
                        &mut result_temp,
                        &mut result_ty_id,
                        &mut emit_ok,
                        diags,
                    )
                };

            if let Some(guard) = &arm.guard {
                let gt =
                    self.analyze_expression(guard, parent_function, &arm_scope, diagnostics)?;
                let guard_hir = self.hir_take();
                if !gt.is_unknown() && !gt.is_bool() {
                    diagnostics.report_error(
                        format!("switch guard must be a bool, got {}", gt.get_type()),
                        guard.position(),
                    );
                }
                self.hir_open_block();
                run_body(self, diagnostics)?;
                let guard_then = self.hir_close_block();
                match guard_hir {
                    Some(g) => self.hir_push_stmt(HStmt::If {
                        cond: g,
                        then_branch: guard_then,
                        else_branch: vec![],
                    }),
                    None => emit_ok = false,
                }
            } else {
                run_body(self, diagnostics)?;
            }
            let then_branch = self.hir_close_block();

            // cond = !done && conds[0] && conds[1] ...
            let mut cond =
                self.hx_not(self.hx_local(done_local.unwrap_or(dream_hir::LocalId(0)), bool_ty));
            for c in conds {
                cond = self.hx_bin(BinOp::And, cond, c);
            }
            self.hir_push_stmt(HStmt::If {
                cond,
                then_branch,
                else_branch: vec![],
            });

            // Track the first irrefutable (catch-all) arm so later arms can be flagged unreachable.
            // (Exhaustiveness itself is decided from the arm patterns in `check_exhaustiveness`.)
            if arm.guard.is_none() && info.irrefutable {
                catch_all_index = Some(i);
            }
        }

        if is_expression {
            match (result_temp, result_ty_id) {
                (Some(tmp), Some(ty)) if emit_ok => self.hir_set_local_read(tmp, ty),
                _ => {
                    self.hir_fail();
                    self.hir_none();
                }
            }
        } else if !emit_ok {
            self.hir_fail();
        }

        // Exhaustiveness: a guarded catch-all doesn't count, so require full variant coverage or `_`
        // (recursively, so nested patterns like `Wrap(A(n))` + `Wrap(B)` count as covering `Wrap`).
        self.check_exhaustiveness(
            &subject_base,
            &subject_type,
            &union_info,
            arms,
            subject.position(),
            diagnostics,
        );

        if is_expression {
            Ok(arm_value_type.unwrap_or(Type::Void))
        } else {
            Ok(Type::Void)
        }
    }
}
