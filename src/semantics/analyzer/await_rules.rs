//! Placement rules for `await`. Inside an `async` function `await` may appear anywhere — including
//! conditionally-evaluated positions (ternary arms, the right operand of `&&`/`||`/`??`) and inside
//! loop/branch/`switch` bodies and conditions — because the coroutine transform lowers the whole
//! body to a CFG state machine in which each `await` is a suspend point (`mir::lower::lower_await`).
//! The only remaining rule is that `await` outside an `async` function is an error.

use super::Analyzer;
use crate::diagnostics::DiagnosticBag;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, StatementNode};

impl<'a> Analyzer<'a> {
    /// Awaiting is allowed anywhere inside an `async` function; in a non-async function every
    /// `await` (however deeply nested) is reported as an error.
    pub(super) fn check_await_positions(
        &self,
        function: &FunctionNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if function.is_async {
            return;
        }
        for stmt in function.body.iter() {
            self.forbid_await_in_stmt(
                stmt,
                "'await' can only be used inside an 'async' function",
                diagnostics,
            );
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
            | ExpressionNode::IsExpression(e, _, _) => {
                self.scan_expr_await(e, message, diagnostics)
            }
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
