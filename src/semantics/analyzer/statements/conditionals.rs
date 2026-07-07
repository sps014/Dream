//! `if`/`else if`/`else` analysis, including the compile-time `is` fold that prunes dead branches
//! and the `is`-with-binding flow typing declared into the taken branch.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::hir::{HExpr, HStmt};
use crate::semantics::errors::SemanticError;
use crate::syntax::nodes::types::strip_nullable;
use crate::syntax::nodes::{ExpressionNode, StatementNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;

impl<'a> Analyzer<'a> {
    pub(in crate::semantics::analyzer) fn analyze_if_else(
        &mut self,
        statement: &StatementNode<'a>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        has_parent_while: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let StatementNode::IfElse(condition, if_body, else_if, else_body) = statement else {
            unreachable!()
        };
        // Live branches of the chain in source order, each `(condition HIR, body)`. An `is` condition
        // on a concrete (non-`object`) operand folds to a compile-time constant: a `false` branch is
        // dead (skipped entirely, so its body — valid only under other instantiations — is never
        // type-checked), and a `true` branch is unconditionally taken, becoming the terminal `else`
        // and ending the chain. Regular conditions are analyzed normally and keep their HIR.
        let mut arms: Vec<(HExpr, Vec<HStmt>)> = Vec::new();
        let mut terminal: Vec<HStmt> = Vec::new();
        let mut terminated = false;

        // Every branch of the chain (primary, then each `else if`) as `(condition, position, body)`.
        let branches = std::iter::once((condition, condition.position(), if_body))
            .chain(else_if.iter().map(|i| (&i.0, i.0.position(), &i.1)));

        for (cond_expr, cond_pos, body) in branches {
            // An `is`-with-binding condition declares a narrowed local `name: T` scoped to the taken
            // branch only. This covers a bare `if (x is T name)` and every `is`-binding reachable
            // through a top-level `&&` chain (`if (a && x is T name)`), each of which is guaranteed to
            // hold in the then-branch. Both the compile-time fold and the runtime path introduce them
            // into that branch's scope.
            let mut bindings: Vec<(&SyntaxToken, &Type, &ExpressionNode<'a>)> = Vec::new();
            Self::collect_is_bindings(cond_expr, &mut bindings);

            // `is` fold: an operand with a concrete (non-`object`, non-interface) static type resolves
            // at compile time, so a branch is either taken unconditionally or is dead. An `object` or
            // interface operand needs a runtime tag check, so it falls through to the general
            // (runtime-`IsType`) path below.
            if let ExpressionNode::IsExpression(left, right_type, _) = cond_expr {
                let left_t = self
                    .analyze_expression(left, ctx.parent_function, ctx.symbol_table, diagnostics)
                    .unwrap_or(Type::Unknown);
                let left_name = strip_nullable(&left_t.get_type()).to_string();
                let runtime =
                    left_t.is_object() || left_t.is_unknown() || self.is_interface_name(&left_name);
                if !runtime {
                    if left_t.get_type() == right_type.get_type() {
                        let branch_scope = self.branch_scope(ctx.symbol_table);
                        self.hir_open_block();
                        self.declare_is_bindings(&bindings, &branch_scope, ctx, diagnostics)?;
                        self.analyze_body(
                            body,
                            ctx.parent_function,
                            Some(&branch_scope),
                            has_parent_while,
                            diagnostics,
                        )?;
                        terminal = self.hir_close_block();
                        terminated = true;
                        break;
                    } else {
                        // Dead branch: skip it entirely (do not analyze its body).
                        continue;
                    }
                }
            }

            let cond_type = self
                .analyze_expression(
                    cond_expr,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                )
                .unwrap_or(Type::Unknown);
            let cond_hir = self.hir_take();
            self.check_bool_condition("if", &cond_type, cond_pos, diagnostics);
            let branch_scope = self.branch_scope(ctx.symbol_table);
            self.hir_open_block();
            self.declare_is_bindings(&bindings, &branch_scope, ctx, diagnostics)?;
            self.analyze_body(
                body,
                ctx.parent_function,
                Some(&branch_scope),
                has_parent_while,
                diagnostics,
            )?;
            let body_hir = self.hir_close_block();
            match cond_hir {
                Some(cond_hir) => arms.push((cond_hir, body_hir)),
                None => self.hir_fail(),
            }
        }

        if !terminated {
            if let Some(body) = else_body {
                self.hir_open_block();
                self.analyze_body(
                    body,
                    ctx.parent_function,
                    Some(ctx.symbol_table),
                    has_parent_while,
                    diagnostics,
                )?;
                terminal = self.hir_close_block();
            }
        }

        // Fold the live arms (innermost last) into a single nested `if`/`else` and emit it.
        let mut chain = terminal;
        for (cond, body) in arms.into_iter().rev() {
            chain = vec![crate::hir::HStmt::If {
                cond,
                then_branch: body,
                else_branch: chain,
            }];
        }
        for stmt in chain {
            self.hir_push_stmt(stmt);
        }
        Ok(())
    }
}
