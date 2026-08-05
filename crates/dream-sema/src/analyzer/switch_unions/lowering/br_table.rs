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
    ExpressionNode, FunctionNode, PatternNode, SwitchArm, Type,
};
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Analyzes a pattern-matching `switch`. `is_expression` is true when the switch is used in
    /// value position (all arms must be `=> expr` and share one type); false in statement position
    /// (block arms are allowed and the result is `void`). Returns the unified arm type (or `void`).
    pub(crate) fn analyze_pattern_switch(
        &mut self,
        subject: &ExpressionNode<'a>,
        arms: &[SwitchArm<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        // Or-patterns and small literal ranges expand into flat multi-key Switch arms. Unexpanded
        // ranges still need the full if-chain. Nested/guards use the hybrid outer-Switch path
        // (residual if-chain inside each Switch arm) when every arm has an outer key.
        let expanded = Self::expand_switch_arms_for_fast_path(arms);
        if Self::pattern_switch_needs_full_chain(&expanded) {
            return self.analyze_pattern_switch_chain(
                subject,
                arms,
                parent_function,
                symbol_table,
                is_expression,
                diagnostics,
            );
        }
        if Self::pattern_switch_needs_residual(&expanded) {
            // A catch-all (`_` / whole-bind) may need to run after a failed nested residual for the
            // same outer key (e.g. `Both(Some(5), None)` after `Both(Some(0), None)`). Outer Switch
            // arms cannot fall through to default once entered, so keep the full if-chain whenever a
            // catch-all is present.
            let has_catch_all = expanded.iter().any(|a| {
                matches!(
                    &a.pattern,
                    PatternNode::Wildcard(_) | PatternNode::Binding(_)
                ) && a.guard.is_none()
            });
            if has_catch_all {
                return self.analyze_pattern_switch_chain(
                    subject,
                    arms,
                    parent_function,
                    symbol_table,
                    is_expression,
                    diagnostics,
                );
            }
            return self.analyze_pattern_switch_hybrid(
                subject,
                arms,
                parent_function,
                symbol_table,
                is_expression,
                diagnostics,
            );
        }
        let (subject_type, subject_hir, subject_base, union_info) =
            self.resolve_switch_subject(subject, parent_function, symbol_table, diagnostics)?;
        let union_def = self
            .type_ctx
            .defs
            .lookup(dream_types::DefKind::Union, &subject_base);

        // Or-alternatives are validated binding-free against the original arms (expansion turns each
        // alt into its own arm, which would otherwise miss this check).
        for arm in arms {
            if let PatternNode::Or(alts) = &arm.pattern {
                for alt in alts {
                    if Self::pattern_introduces_binding(alt, &union_info) {
                        diagnostics.report_error(
                            "an or-pattern alternative cannot bind a variable; use `_`, a literal, a range, or a payload-free variant".to_string(),
                            alt.position(),
                        );
                    }
                }
            }
        }

        // A whole-subject binding arm (`other => ...`, where `other` names no unit variant) needs the
        // subject value available in the `default` block. Bind it to a temp once and dispatch the
        // `Switch` on a read of that temp, so the binding arm can copy it into its named local.
        let subj_ty_id = self.type_ctx.lower(&subject_type);
        let has_whole_bind = expanded.iter().any(|a| {
            a.guard.is_none()
                && matches!(&a.pattern, PatternNode::Binding(n)
                    if !matches!(&union_info, Some(info) if info.variant(&n.text).is_some_and(|v| v.fields.is_empty())))
        });
        let switch_scrutinee = if has_whole_bind {
            match (
                self.hir_alloc_local("__switch_subj", &subject_type),
                subject_hir,
            ) {
                (Some(subj_local), Some(sh)) => {
                    self.hir_push_stmt(dream_hir::HStmt::Let {
                        local: subj_local,
                        ty: subj_ty_id,
                        value: sh,
                    });
                    Some(self.hx_local(subj_local, subj_ty_id))
                }
                _ => None,
            }
        } else {
            subject_hir
        };

        let mut arm_value_type: Option<Type> = None;
        let mut catch_all_index: Option<usize> = None;

        // HIR: build `Switch` arms + a default block. A statement-position switch lowers directly; a
        // value-position switch desugars to `<result temp> = arm; … ; <result temp read>`, with each
        // arm body assigning the shared result temporary.
        let mut hir_arms: Vec<dream_hir::HArm> = Vec::new();
        let mut hir_default: Vec<dream_hir::HStmt> = Vec::new();
        let mut hir_ok = switch_scrutinee.is_some();
        let mut result_temp: Option<dream_hir::LocalId> = None;
        let mut result_ty_id: Option<dream_types::TypeId> = None;

        for (i, arm) in expanded.iter().enumerate() {
            if catch_all_index.is_some() {
                diagnostics.report_error(
                    "Unreachable switch arm: a previous arm already matches everything".to_string(),
                    arm.pattern.position(),
                );
            }

            // Each arm introduces its pattern bindings into a fresh child scope.
            let arm_scope = Rc::new(RefCell::new(SymbolTable::new(Some(symbol_table.clone()))));
            (*symbol_table).borrow_mut().add_child(arm_scope.clone());

            let info = self.check_pattern(&arm.pattern, &subject_type, &arm_scope, diagnostics)?;

            // No arm here has a guard or nested residual: those were routed to hybrid/full-chain
            // above, so a guard can never actually appear on an arm this loop analyzes.
            debug_assert!(
                arm.guard.is_none(),
                "a guarded arm reached the flat Switch path; pattern_switch_needs_residual should have routed it to analyze_pattern_switch_hybrid"
            );

            // Classify the pattern (allocating payload binding slots) before the body is lowered.
            let shape =
                self.hir_switch_pattern(&arm.pattern, &union_info, union_def, &subject_type);
            debug_assert!(
                !matches!(shape, HirArmShape::Unsupported),
                "expand_switch_arms_for_fast_path + routing should keep Unsupported off the flat Switch path"
            );

            self.hir_open_block();
            // A whole-subject binding copies the subject into its named local as the first statement
            // of the (catch-all) body, mirroring the `Switch` scrutinee read.
            if let HirArmShape::DefaultBind { local, ty } = &shape {
                let read = self.hx_local(
                    match &switch_scrutinee {
                        Some(dream_hir::HExpr {
                            kind: dream_hir::HExprKind::Var(dream_hir::Binding::Local(l)),
                            ..
                        }) => *l,
                        _ => dream_hir::LocalId(0),
                    },
                    subj_ty_id,
                );
                self.hir_push_stmt(dream_hir::HStmt::Let {
                    local: *local,
                    ty: *ty,
                    value: read,
                });
            }
            self.analyze_switch_arm_result(
                arm,
                parent_function,
                &arm_scope,
                is_expression,
                &mut arm_value_type,
                &mut result_temp,
                &mut result_ty_id,
                &mut hir_ok,
                diagnostics,
            )?;
            let body_hir = self.hir_close_block();

            match shape {
                HirArmShape::Default | HirArmShape::DefaultBind { .. } => hir_default = body_hir,
                HirArmShape::Const(label) => match self.hir_const_arm(Some(label), body_hir) {
                    Some(arm) => hir_arms.push(arm),
                    None => hir_ok = false,
                },
                HirArmShape::Variant {
                    def,
                    variant,
                    bindings,
                } => hir_arms.push(self.hir_variant_arm(def, variant, bindings, body_hir)),
                HirArmShape::Unsupported => hir_ok = false,
            }

            // Track the first irrefutable (catch-all) arm so later arms can be flagged unreachable.
            // (Exhaustiveness itself is decided from the arm patterns in `check_exhaustiveness`.)
            if arm.guard.is_none() && info.irrefutable {
                catch_all_index = Some(i);
            }
        }

        if is_expression {
            // Emit the desugared switch, then leave the result temp read as the match's value.
            match (result_temp, result_ty_id) {
                (Some(tmp), Some(ty)) if hir_ok => {
                    self.hir_switch(switch_scrutinee, hir_arms, hir_default, true);
                    self.hir_set_local_read(tmp, ty);
                }
                _ => {
                    self.hir_fail();
                    self.hir_none();
                }
            }
        } else {
            self.hir_switch(switch_scrutinee, hir_arms, hir_default, hir_ok);
        }

        // Exhaustiveness uses the original arms so Or/Range coverage is computed as written.
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
