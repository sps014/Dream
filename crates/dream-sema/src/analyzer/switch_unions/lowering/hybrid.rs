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
use crate::union_table::UnionInfo;
use dream_syntax::nodes::{
    ExpressionNode, FunctionNode, PatternNode, SwitchArm, Type,
};
use dream_syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Hybrid pattern-`switch` lowering: an outer `HStmt::Switch`/`br_table` on each arm's outer
    /// key (variant tag / const), with a residual if-chain inside each Switch arm for nested
    /// sub-patterns and guards. Used when every arm has a Switch-representable outer key but at
    /// least one needs residual work (see [`Analyzer::pattern_switch_needs_residual`]).
    pub(crate) fn analyze_pattern_switch_hybrid(
        &mut self,
        subject: &ExpressionNode<'a>,
        arms: &[SwitchArm<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        use dream_hir::{BinOp, HStmt};
        use indexmap::IndexMap;

        let (subject_type, subject_hir, subject_base, union_info) =
            self.resolve_switch_subject(subject, parent_function, symbol_table, diagnostics)?;
        let union_def = self
            .type_ctx
            .defs
            .lookup(dream_types::DefKind::Union, &subject_base);

        let bool_type = Type::Boolean(synthetic_token(TokenKind::BooleanToken, "bool"));
        let bool_ty = self.type_ctx.interner.bool();
        let subj_ty_id = self.type_ctx.lower(&subject_type);

        let mut emit_ok = subject_hir.is_some();
        let subj_local = self.hir_alloc_local("__switch_subj", &subject_type);
        match (subj_local, subject_hir) {
            (Some(local), Some(sh)) => self.hir_push_stmt(HStmt::Let {
                local,
                ty: subj_ty_id,
                value: sh,
            }),
            _ => emit_ok = false,
        }

        let subj_read =
            |s: &Self| s.hx_local(subj_local.unwrap_or(dream_hir::LocalId(0)), subj_ty_id);
        let switch_scrutinee = Some(subj_read(self));

        // Group arms by outer Switch key (variant name / const spelling / catch-all), preserving
        // first-seen order via IndexMap.
        let mut groups: IndexMap<String, Vec<&SwitchArm<'a>>> = IndexMap::new();
        for arm in arms {
            let key = Self::outer_switch_key(&arm.pattern, &union_info);
            groups.entry(key).or_default().push(arm);
        }

        let mut arm_value_type: Option<Type> = None;
        let mut catch_all_index: Option<usize> = None;
        let mut result_temp: Option<dream_hir::LocalId> = None;
        let mut result_ty_id: Option<dream_types::TypeId> = None;
        let mut hir_arms: Vec<dream_hir::HArm> = Vec::new();
        let mut hir_default: Vec<HStmt> = Vec::new();

        for (gi, (_key, group)) in groups.iter().enumerate() {
            // Type-check every arm in the group (scopes, exhaustiveness inputs).
            for arm in group {
                if catch_all_index.is_some() {
                    diagnostics.report_error(
                        "Unreachable switch arm: a previous arm already matches everything"
                            .to_string(),
                        arm.pattern.position(),
                    );
                }
                let arm_scope =
                    Rc::new(RefCell::new(SymbolTable::new(Some(symbol_table.clone()))));
                (*symbol_table).borrow_mut().add_child(arm_scope.clone());
                let info =
                    self.check_pattern(&arm.pattern, &subject_type, &arm_scope, diagnostics)?;
                if arm.guard.is_none() && info.irrefutable {
                    catch_all_index = Some(gi);
                }
            }

            // Residual if-chain body for this outer key (done flag is local to the Switch arm).
            self.hir_open_block();
            let done_local = self.hir_alloc_local("__switch_done", &bool_type);
            if let Some(done) = done_local {
                let init = self.hx_bool(false);
                self.hir_push_stmt(HStmt::Let {
                    local: done,
                    ty: bool_ty,
                    value: init,
                });
            }

            for arm in group {
                let arm_scope =
                    Rc::new(RefCell::new(SymbolTable::new(Some(symbol_table.clone()))));
                (*symbol_table).borrow_mut().add_child(arm_scope.clone());
                let _ = self.check_pattern(&arm.pattern, &subject_type, &arm_scope, diagnostics)?;

                let sr = subj_read(self);
                let (conds, binds) = match self.compile_pattern(&sr, &subject_type, &arm.pattern) {
                    Some(cb) => cb,
                    None => {
                        emit_ok = false;
                        (vec![], vec![])
                    }
                };

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
                    self.hir_open_block();
                    let gt =
                        self.analyze_expression(guard, parent_function, &arm_scope, diagnostics)?;
                    let guard_hir = self.hir_take();
                    if !gt.is_bool() && !gt.is_unknown() {
                        diagnostics.report_error(
                            format!("switch arm guard must be bool, got {}", gt.get_type()),
                            guard.position(),
                        );
                    }
                    run_body(self, diagnostics)?;
                    let then_b = self.hir_close_block();
                    match guard_hir {
                        Some(gh) => self.hir_push_stmt(HStmt::If {
                            cond: gh,
                            then_branch: then_b,
                            else_branch: vec![],
                        }),
                        None => emit_ok = false,
                    }
                } else {
                    run_body(self, diagnostics)?;
                }
                let then_body = self.hir_close_block();

                // `if (!done && conds…) { … }`
                let mut gate = match done_local {
                    Some(done) => {
                        let done_read = self.hx_local(done, bool_ty);
                        Some(self.hx_not(done_read))
                    }
                    None => None,
                };
                for c in conds {
                    gate = Some(match gate {
                        Some(g) => self.hx_bin(BinOp::And, g, c),
                        None => c,
                    });
                }
                if let Some(g) = gate {
                    self.hir_push_stmt(HStmt::If {
                        cond: g,
                        then_branch: then_body,
                        else_branch: vec![],
                    });
                } else {
                    for s in then_body {
                        self.hir_push_stmt(s);
                    }
                }
            }

            let body_hir = self.hir_close_block();

            // Flat outer pattern for the Switch arm (first arm's pattern with bindings/wildcards).
            let outer_pat = Self::flatten_outer_pattern(&group[0].pattern);
            let shape =
                self.hir_switch_pattern(&outer_pat, &union_info, union_def, &subject_type);
            match shape {
                HirArmShape::Default | HirArmShape::DefaultBind { .. } => hir_default = body_hir,
                HirArmShape::Const(label) => match self.hir_const_arm(Some(label), body_hir) {
                    Some(arm) => hir_arms.push(arm),
                    None => emit_ok = false,
                },
                HirArmShape::Variant {
                    def,
                    variant,
                    bindings,
                } => hir_arms.push(self.hir_variant_arm(def, variant, bindings, body_hir)),
                HirArmShape::Unsupported => {
                    // Fall back: append residual body as default-like fallthrough.
                    hir_default.extend(body_hir);
                    emit_ok = false;
                }
            }
        }

        if is_expression {
            match (result_temp, result_ty_id) {
                (Some(tmp), Some(ty)) if emit_ok => {
                    self.hir_switch(switch_scrutinee, hir_arms, hir_default, true);
                    self.hir_set_local_read(tmp, ty);
                }
                _ => {
                    self.hir_fail();
                    self.hir_none();
                }
            }
        } else {
            self.hir_switch(switch_scrutinee, hir_arms, hir_default, emit_ok);
        }

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
    /// Grouping key for hybrid Switch arms: variant name, const spelling, or `"_"` catch-all.
    pub(crate) fn outer_switch_key(pattern: &PatternNode, union_info: &Option<UnionInfo>) -> String {
        match pattern {
            PatternNode::Variant(_, name, _) => format!("v:{}", name.text),
            PatternNode::Literal(lit) => format!("c:{}", lit.display_name()),
            PatternNode::Wildcard(_) => "_".to_string(),
            PatternNode::Binding(n) => {
                if matches!(union_info, Some(info) if info.variant(&n.text).is_some_and(|v| v.fields.is_empty()))
                {
                    format!("v:{}", n.text)
                } else {
                    "_".to_string()
                }
            }
            PatternNode::Or(alts) => alts
                .first()
                .map(|a| Self::outer_switch_key(a, union_info))
                .unwrap_or_else(|| "_".to_string()),
            PatternNode::Range(..) => "_".to_string(),
        }
    }
    /// Strips nested sub-patterns to Binding/Wildcard so [`hir_switch_pattern`] accepts the arm.
    pub(crate) fn flatten_outer_pattern(pattern: &PatternNode) -> PatternNode {
        match pattern {
            PatternNode::Variant(qual, name, subs) => {
                let flat: Vec<PatternNode> = subs
                    .iter()
                    .map(|s| match s {
                        PatternNode::Binding(t) => PatternNode::Binding(t.clone()),
                        _ => PatternNode::Wildcard(name.clone()),
                    })
                    .collect();
                PatternNode::Variant(qual.clone(), name.clone(), flat)
            }
            other => other.clone(),
        }
    }
}
