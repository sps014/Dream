//! The pattern-`switch` lowering paths, plus the subject-resolution/arm-result helpers they share:
//! - [`Analyzer::analyze_pattern_switch`]: the `Switch`/br_table fast path for flat, unguarded
//!   variant/const/catch-all arms (including or-patterns and small literal ranges expanded into
//!   multi-key arms).
//! - [`Analyzer::analyze_pattern_switch_hybrid`]: outer `Switch` plus residual if-chain inside each
//!   arm for nested/literal sub-patterns and guards.
//! - [`Analyzer::analyze_pattern_switch_chain`]: full if-chain fallback when no outer Switch key
//!   exists (unexpanded ranges; see [`Analyzer::pattern_switch_needs_full_chain`]).

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::semantics::union_table::UnionInfo;
use crate::syntax::nodes::{
    ExpressionNode, FunctionNode, PatternNode, SwitchArm, SwitchArmBody, Type,
};
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Resolves a pattern-`switch`'s subject expression, ensuring any generic union it names is
    /// instantiated (so its layout/variants are known before arm patterns are checked against it),
    /// and returns its type, lowered HIR, and (name, union info) for arm/exhaustiveness analysis.
    /// Shared by both switch-lowering paths ([`Self::analyze_pattern_switch`], which emits a real
    /// HIR `Switch`/br_table, and [`Self::analyze_pattern_switch_chain`], the general if-chain
    /// fallback for guarded/nested patterns) so they cannot drift on how the subject is set up.
    fn resolve_switch_subject(
        &mut self,
        subject: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(Type, Option<crate::hir::HExpr>, String, Option<UnionInfo>), SemanticError> {
        let subject_type =
            self.analyze_expression(subject, parent_function, symbol_table, diagnostics)?;
        let subject_hir = self.hir_take();
        // The subject's union may be a generic instantiation that has not been constructed yet
        // (e.g. matching on a `param: Option<int>`); ensure its layout is registered first.
        if let Type::Struct(base, Some(args)) = &subject_type {
            if self.generic_unions.contains_key(&base.text) {
                self.ensure_union_instantiated(&base.text, args, &base.position, diagnostics);
            }
        }
        let subject_base = subject_type.get_type();
        let union_info: Option<UnionInfo> = self.union_table.get(&subject_base).cloned();
        Ok((subject_type, subject_hir, subject_base, union_info))
    }

    /// Analyzes one switch arm's body (`=> expr` or `=> { stmts }`) and, in expression position,
    /// desugars it into an assignment to the shared `__switch_result` temp (allocated from the
    /// first arm's type; later arms are unified against it). Shared by both switch-lowering paths,
    /// which previously each carried their own copy of this ~30-line desugaring. The caller owns
    /// `hir_open_block`/`hir_close_block` around this call so it can inject its own arm-body prefix
    /// statement first (a whole-subject-binding `let` for the `Switch` path, a `done = true` flag
    /// assignment for the if-chain path).
    #[allow(clippy::too_many_arguments)]
    fn analyze_switch_arm_result(
        &mut self,
        arm: &SwitchArm<'a>,
        parent_function: &FunctionNode<'a>,
        arm_scope: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        arm_value_type: &mut Option<Type>,
        result_temp: &mut Option<crate::hir::LocalId>,
        result_ty_id: &mut Option<crate::types::TypeId>,
        ok: &mut bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        match &arm.body {
            SwitchArmBody::Expr(expr) => {
                let t = self.analyze_expression(expr, parent_function, arm_scope, diagnostics)?;
                let arm_hir = self.hir_take();
                if is_expression {
                    match arm_value_type {
                        None => *arm_value_type = Some(t.clone()),
                        Some(prev) => {
                            self.compare_data_type(prev, &t, &empty_span(), diagnostics)?
                        }
                    }
                    if result_temp.is_none() {
                        *result_temp = self.hir_alloc_local("__switch_result", &t);
                        *result_ty_id = Some(self.type_ctx.lower(&t));
                    }
                    match *result_temp {
                        Some(tmp) => self.hir_assign_local_id(tmp, arm_hir),
                        None => *ok = false,
                    }
                } else {
                    self.hir_expr_stmt(arm_hir);
                }
            }
            SwitchArmBody::Block(stmts) => {
                if is_expression {
                    diagnostics.report_error(
                        "A block arm (`=> { ... }`) is only allowed when `switch` is used as a statement; use `=> expr` in expression position".to_string(),
                        arm.pattern.position(),
                    );
                }
                self.analyze_body(stmts, parent_function, Some(arm_scope), false, diagnostics)?;
            }
        }
        Ok(())
    }

    /// General pattern-`switch` lowering (guards + nested/literal sub-patterns) as a flag-gated
    /// if-chain: evaluates the subject once, then for each arm emits `if (!done && <tests>) {
    /// <binds>; [if (<guard>)] { done = true; <body> } }`. A failed guard leaves `done` false so the
    /// next arm is tried. Type-checking (pattern checks, guard/body analysis, exhaustiveness) mirrors
    /// the `Switch` path.
    fn analyze_pattern_switch_chain(
        &mut self,
        subject: &ExpressionNode<'a>,
        arms: &[SwitchArm<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        use crate::hir::{BinOp, HStmt};

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
            |s: &Self| s.hx_local(subj_local.unwrap_or(crate::hir::LocalId(0)), subj_ty_id);

        let mut arm_value_type: Option<Type> = None;
        let mut catch_all_index: Option<usize> = None;
        let mut result_temp: Option<crate::hir::LocalId> = None;
        let mut result_ty_id: Option<crate::types::TypeId> = None;

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
                self.hx_not(self.hx_local(done_local.unwrap_or(crate::hir::LocalId(0)), bool_ty));
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

    /// Hybrid pattern-`switch` lowering: an outer `HStmt::Switch`/`br_table` on each arm's outer
    /// key (variant tag / const), with a residual if-chain inside each Switch arm for nested
    /// sub-patterns and guards. Used when every arm has a Switch-representable outer key but at
    /// least one needs residual work (see [`Analyzer::pattern_switch_needs_residual`]).
    fn analyze_pattern_switch_hybrid(
        &mut self,
        subject: &ExpressionNode<'a>,
        arms: &[SwitchArm<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        use crate::hir::{BinOp, HStmt};
        use indexmap::IndexMap;

        let (subject_type, subject_hir, subject_base, union_info) =
            self.resolve_switch_subject(subject, parent_function, symbol_table, diagnostics)?;
        let union_def = self
            .type_ctx
            .defs
            .lookup(crate::types::DefKind::Union, &subject_base);

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
            |s: &Self| s.hx_local(subj_local.unwrap_or(crate::hir::LocalId(0)), subj_ty_id);
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
        let mut result_temp: Option<crate::hir::LocalId> = None;
        let mut result_ty_id: Option<crate::types::TypeId> = None;
        let mut hir_arms: Vec<crate::hir::HArm> = Vec::new();
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
    fn outer_switch_key(pattern: &PatternNode, union_info: &Option<UnionInfo>) -> String {
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
    fn flatten_outer_pattern(pattern: &PatternNode) -> PatternNode {
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

    /// Analyzes a pattern-matching `switch`. `is_expression` is true when the switch is used in
    /// value position (all arms must be `=> expr` and share one type); false in statement position
    /// (block arms are allowed and the result is `void`). Returns the unified arm type (or `void`).
    pub(in crate::semantics::analyzer) fn analyze_pattern_switch(
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
            .lookup(crate::types::DefKind::Union, &subject_base);

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
                    self.hir_push_stmt(crate::hir::HStmt::Let {
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
        let mut hir_arms: Vec<crate::hir::HArm> = Vec::new();
        let mut hir_default: Vec<crate::hir::HStmt> = Vec::new();
        let mut hir_ok = switch_scrutinee.is_some();
        let mut result_temp: Option<crate::hir::LocalId> = None;
        let mut result_ty_id: Option<crate::types::TypeId> = None;

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
                        Some(crate::hir::HExpr {
                            kind: crate::hir::HExprKind::Var(crate::hir::Binding::Local(l)),
                            ..
                        }) => *l,
                        _ => crate::hir::LocalId(0),
                    },
                    subj_ty_id,
                );
                self.hir_push_stmt(crate::hir::HStmt::Let {
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
