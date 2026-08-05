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
    ExpressionNode, FunctionNode, SwitchArm, SwitchArmBody, Type,
};
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Resolves a pattern-`switch`'s subject expression, ensuring any generic union it names is
    /// instantiated (so its layout/variants are known before arm patterns are checked against it),
    /// and returns its type, lowered HIR, and (name, union info) for arm/exhaustiveness analysis.
    /// Shared by both switch-lowering paths ([`Self::analyze_pattern_switch`], which emits a real
    /// HIR `Switch`/br_table, and [`Self::analyze_pattern_switch_chain`], the general if-chain
    /// fallback for guarded/nested patterns) so they cannot drift on how the subject is set up.
    pub(crate) fn resolve_switch_subject(
        &mut self,
        subject: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(Type, Option<dream_hir::HExpr>, String, Option<UnionInfo>), SemanticError> {
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
    pub(crate) fn analyze_switch_arm_result(
        &mut self,
        arm: &SwitchArm<'a>,
        parent_function: &FunctionNode<'a>,
        arm_scope: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        arm_value_type: &mut Option<Type>,
        result_temp: &mut Option<dream_hir::LocalId>,
        result_ty_id: &mut Option<dream_types::TypeId>,
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
}
