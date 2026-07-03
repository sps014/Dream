//! Placement rules for `await`. Inside an `async` function, `await` may appear in any
//! *unconditionally evaluated* sub-expression of a top-level statement (`let x = f(await a());`,
//! `g(await a(), await b());`, `return (await a()) + 1;`, etc.); the MIR async normalization pass
//! (`mir::async_normalize`) hoists those into top-level `let` temporaries before the coroutine
//! transform runs. Awaits in *conditionally* evaluated positions — the arms of a ternary, the right
//! operand of `&&`/`||`/`??`, and anywhere inside loop/branch bodies or loop conditions — are still
//! rejected here, since suspending there would require a full CFG state machine. Awaiting anywhere
//! in a non-`async` function is also rejected.

use super::Analyzer;
use crate::diagnostics::DiagnosticBag;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, StatementNode};
use crate::syntax::token::token_kind::TokenKind;

const CONDITIONAL_AWAIT_MSG: &str =
    "'await' cannot appear in a conditionally-evaluated position (a ternary arm, the right side of '&&'/'||'/'??', or inside a loop/branch body or loop condition); bind it to a variable first (e.g. `let x = await e;`)";

impl<'a> Analyzer<'a> {
    /// Awaits in conditionally-evaluated positions, loop/branch bodies, or non-async functions are
    /// rejected; awaits in unconditional sub-expressions of a top-level statement are allowed.
    pub(super) fn check_await_positions(
        &self,
        function: &FunctionNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !function.is_async {
            for stmt in function.body.iter() {
                self.forbid_await_in_stmt(
                    stmt,
                    "'await' can only be used inside an 'async' function",
                    diagnostics,
                );
            }
            return;
        }
        for stmt in function.body.iter() {
            self.check_await_in_top_stmt(stmt, diagnostics);
        }
    }

    /// Checks a top-level statement of an async function: awaits are permitted in the statement's
    /// unconditionally-evaluated expressions, but rejected inside control-flow statements (whose
    /// bodies/conditions cannot yet suspend).
    fn check_await_in_top_stmt(
        &self,
        stmt: &StatementNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        match stmt {
            StatementNode::Declaration(_, _, e, _)
            | StatementNode::Assignment(_, e)
            | StatementNode::ExpressionStatement(e)
            | StatementNode::AwaitStmt(e) => self.check_expr_await(e, false, diagnostics),
            StatementNode::Return(Some(e)) => self.check_expr_await(e, false, diagnostics),
            StatementNode::MemberAssignment(obj, _, e) => {
                self.check_expr_await(obj, false, diagnostics);
                self.check_expr_await(e, false, diagnostics);
            }
            StatementNode::IndexAssignment(arr, idx, e) => {
                self.check_expr_await(arr, false, diagnostics);
                self.check_expr_await(idx, false, diagnostics);
                self.check_expr_await(e, false, diagnostics);
            }
            StatementNode::FunctionInvocation(_, _, args) => {
                for a in args {
                    self.check_expr_await(a, false, diagnostics);
                }
            }
            StatementNode::MethodInvocation(obj, _, _, args) => {
                self.check_expr_await(obj, false, diagnostics);
                for a in args {
                    self.check_expr_await(a, false, diagnostics);
                }
            }
            // Control-flow statements cannot yet suspend across their conditions/bodies.
            other => self.forbid_await_in_stmt(
                other,
                "'await' is not supported inside loops or branches yet; move it to a top-level statement (e.g. `let x = await e;`)",
                diagnostics,
            ),
        }
    }

    /// Walks `expr`, allowing awaits in unconditionally-evaluated positions and reporting an error
    /// for any await reached through a conditionally-evaluated one. `conditional` is true once the
    /// walk has descended into a ternary arm or the short-circuiting right operand of `&&`/`||`/`??`.
    fn check_expr_await(
        &self,
        expr: &ExpressionNode<'a>,
        conditional: bool,
        diagnostics: &mut DiagnosticBag,
    ) {
        match expr {
            ExpressionNode::Await(inner) => {
                if conditional {
                    diagnostics.report_error(CONDITIONAL_AWAIT_MSG.to_string(), inner.position());
                }
                // The awaited future expression is itself unconditional relative to this await.
                self.check_expr_await(inner, conditional, diagnostics);
            }
            ExpressionNode::Binary(l, op, r) => {
                let rhs_conditional = conditional
                    || matches!(
                        op.kind,
                        TokenKind::AmpersandAmpersandToken
                            | TokenKind::PipePipeToken
                            | TokenKind::QuestionQuestionToken
                    );
                self.check_expr_await(l, conditional, diagnostics);
                self.check_expr_await(r, rhs_conditional, diagnostics);
            }
            ExpressionNode::Ternary(c, t, e) => {
                self.check_expr_await(c, conditional, diagnostics);
                self.check_expr_await(t, true, diagnostics);
                self.check_expr_await(e, true, diagnostics);
            }
            ExpressionNode::Unary(_, e)
            | ExpressionNode::Parenthesized(e)
            | ExpressionNode::Cast(_, e)
            | ExpressionNode::IsExpression(e, _, _) => {
                self.check_expr_await(e, conditional, diagnostics)
            }
            ExpressionNode::FunctionCall(_, _, args) => {
                for a in args {
                    self.check_expr_await(a, conditional, diagnostics);
                }
            }
            ExpressionNode::MethodCall(obj, _, _, args) => {
                self.check_expr_await(obj, conditional, diagnostics);
                for a in args {
                    self.check_expr_await(a, conditional, diagnostics);
                }
            }
            ExpressionNode::ArrayLiteral(elems) => {
                for e in elems {
                    self.check_expr_await(e, conditional, diagnostics);
                }
            }
            ExpressionNode::IndexAccess(a, i) => {
                self.check_expr_await(a, conditional, diagnostics);
                self.check_expr_await(i, conditional, diagnostics);
            }
            ExpressionNode::MemberAccess(o, _) => {
                self.check_expr_await(o, conditional, diagnostics)
            }
            // A `switch` expression evaluates its arms conditionally; forbid awaits throughout it
            // (the async normalization does not lower switch-expression awaits).
            ExpressionNode::Switch(subject, arms) => {
                self.check_expr_await(subject, true, diagnostics);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.check_expr_await(g, true, diagnostics);
                    }
                    if let crate::syntax::nodes::SwitchArmBody::Expr(body) = &arm.body {
                        self.check_expr_await(body, true, diagnostics);
                    }
                }
            }
            _ => {}
        }
    }

    /// Reports `message` at every `await` found anywhere inside `stmt` (including nested bodies).
    fn forbid_await_in_stmt(
        &self,
        stmt: &StatementNode<'a>,
        message: &str,
        diagnostics: &mut DiagnosticBag,
    ) {
        match stmt {
            StatementNode::AwaitStmt(inner) => {
                diagnostics.report_error(message.to_string(), inner.position());
                self.scan_expr_await(inner, message, diagnostics);
            }
            StatementNode::Declaration(_, _, e, _)
            | StatementNode::Assignment(_, e)
            | StatementNode::IndexAssignment(_, _, e)
            | StatementNode::ExpressionStatement(e)
            | StatementNode::MemberAssignment(_, _, e) => {
                self.scan_expr_await(e, message, diagnostics);
            }
            StatementNode::Return(Some(e)) => self.scan_expr_await(e, message, diagnostics),
            StatementNode::FunctionInvocation(_, _, args) => {
                for a in args {
                    self.scan_expr_await(a, message, diagnostics);
                }
            }
            StatementNode::MethodInvocation(_, _, _, args) => {
                for a in args {
                    self.scan_expr_await(a, message, diagnostics);
                }
            }
            StatementNode::IfElse(c, b, elifs, eb) => {
                self.scan_expr_await(c, message, diagnostics);
                for s in b.iter() {
                    self.forbid_await_in_stmt(s, message, diagnostics);
                }
                for (ec, eb2) in elifs.iter() {
                    self.scan_expr_await(ec, message, diagnostics);
                    for s in eb2.iter() {
                        self.forbid_await_in_stmt(s, message, diagnostics);
                    }
                }
                if let Some(eb) = eb {
                    for s in eb.iter() {
                        self.forbid_await_in_stmt(s, message, diagnostics);
                    }
                }
            }
            StatementNode::While(c, b) | StatementNode::DoWhile(b, c) => {
                self.scan_expr_await(c, message, diagnostics);
                for s in b.iter() {
                    self.forbid_await_in_stmt(s, message, diagnostics);
                }
            }
            StatementNode::For(init, cond, inc, body) => {
                if let Some(i) = init {
                    self.forbid_await_in_stmt(i, message, diagnostics);
                }
                if let Some(c) = cond {
                    self.scan_expr_await(c, message, diagnostics);
                }
                if let Some(i) = inc {
                    self.forbid_await_in_stmt(i, message, diagnostics);
                }
                for s in body.iter() {
                    self.forbid_await_in_stmt(s, message, diagnostics);
                }
            }
            StatementNode::ForEach(_, iterable, _, _, body) => {
                self.scan_expr_await(iterable, message, diagnostics);
                for s in body.iter() {
                    self.forbid_await_in_stmt(s, message, diagnostics);
                }
            }
            StatementNode::Switch(subject, cases, default_body) => {
                self.scan_expr_await(subject, message, diagnostics);
                for (_, body) in cases.iter() {
                    for s in body.iter() {
                        self.forbid_await_in_stmt(s, message, diagnostics);
                    }
                }
                if let Some(db) = default_body {
                    for s in db.iter() {
                        self.forbid_await_in_stmt(s, message, diagnostics);
                    }
                }
            }
            StatementNode::Labeled(_, inner) => {
                self.forbid_await_in_stmt(inner, message, diagnostics)
            }
            _ => {}
        }
    }

    /// Recursively reports `message` at every nested `await` expression within `expr`.
    fn scan_expr_await(
        &self,
        expr: &ExpressionNode<'a>,
        message: &str,
        diagnostics: &mut DiagnosticBag,
    ) {
        match expr {
            ExpressionNode::Await(inner) => {
                diagnostics.report_error(message.to_string(), inner.position());
                self.scan_expr_await(inner, message, diagnostics);
            }
            ExpressionNode::Binary(l, _, r) => {
                self.scan_expr_await(l, message, diagnostics);
                self.scan_expr_await(r, message, diagnostics);
            }
            ExpressionNode::Unary(_, e)
            | ExpressionNode::Parenthesized(e)
            | ExpressionNode::Cast(_, e)
            | ExpressionNode::IsExpression(e, _, _) => self.scan_expr_await(e, message, diagnostics),
            ExpressionNode::FunctionCall(_, _, args) => {
                for a in args {
                    self.scan_expr_await(a, message, diagnostics);
                }
            }
            ExpressionNode::MethodCall(obj, _, _, args) => {
                self.scan_expr_await(obj, message, diagnostics);
                for a in args {
                    self.scan_expr_await(a, message, diagnostics);
                }
            }
            ExpressionNode::ArrayLiteral(elems) => {
                for e in elems {
                    self.scan_expr_await(e, message, diagnostics);
                }
            }
            ExpressionNode::IndexAccess(a, i) => {
                self.scan_expr_await(a, message, diagnostics);
                self.scan_expr_await(i, message, diagnostics);
            }
            ExpressionNode::MemberAccess(o, _) => self.scan_expr_await(o, message, diagnostics),
            ExpressionNode::Ternary(c, t, e) => {
                self.scan_expr_await(c, message, diagnostics);
                self.scan_expr_await(t, message, diagnostics);
                self.scan_expr_await(e, message, diagnostics);
            }
            _ => {}
        }
    }
}
