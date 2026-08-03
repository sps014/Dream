//! Free-variable analysis for arrow-lambda captures (Milestone B).
//!
//! Capture is **transitive / multi-level**: a lambda nested inside another lambda may reach past
//! its immediate parent to a grandparent's (or higher) local. This falls out of [`lambda_free_names`]
//! descending into a nested lambda's own body rather than treating it as opaque — a name a doubly-
//! nested lambda needs shows up as one of the *outer* lambda's free names too, so `analyze_lambda`
//! forwards it as one of the outer lambda's own captures (received from its own creator, exactly
//! like any other capture), which the inner lambda then captures a second time from the outer one.
//! Each level only ever knows about its own *immediate* captures; the chaining is what reaches
//! further up, one hop at a time.
//!
//! Two entry points:
//! - [`scan_function_captures`] — a whole-function pre-pass, run *before* the function's body is
//!   analyzed (so the enclosing `let`s it finds can be boxed into `__Cell<T>` before they are
//!   emitted — see `Analyzer::boxed_locals`). Finds every lambda anywhere in the body (any nesting
//!   depth) and unions each one's own free names.
//! - [`lambda_free_names`] — one lambda's own free names (referenced but not bound within its own
//!   body), used both by the pre-pass above and again when the lambda itself is analyzed (to build
//!   its capture list / synthesized environment — see `expressions::lambda`).
//!
//! Both deliberately over-approximate rather than track precise lexical scoping/shadowing order: a
//! name is "bound" if it is declared *anywhere* in the relevant body, regardless of position
//! relative to its uses. This can only make the analysis too conservative (boxing/capturing a name
//! that turns out to be shadowed and never really needed), never unsound.

use crate::syntax::nodes::{
    ExpressionNode, LambdaBody, LambdaNode, PatternNode, StatementNode, SwitchArmBody,
};
use std::collections::HashSet;

/// Unions [`lambda_free_names`] over every arrow-lambda anywhere in `stmts` (a whole function
/// body), at any statement/expression nesting depth. Does not descend *into* a found lambda's own
/// body beyond computing its own free names — a lambda nested within it is that lambda's own
/// separate concern (see the module doc comment's capture-scope note), found and scanned
/// separately once that lambda's own turn to be analyzed comes up.
pub(in crate::semantics::analyzer) fn scan_function_captures(
    stmts: &[StatementNode],
) -> HashSet<String> {
    let mut out = HashSet::new();
    walk_stmts_for_lambdas(stmts, &mut out);
    out
}

fn walk_stmts_for_lambdas(stmts: &[StatementNode], out: &mut HashSet<String>) {
    for s in stmts {
        walk_stmt_for_lambdas(s, out);
    }
}

fn walk_stmt_for_lambdas(stmt: &StatementNode, out: &mut HashSet<String>) {
    match stmt {
        StatementNode::Assignment(_, e) => walk_expr_for_lambdas(e, out),
        StatementNode::IndexAssignment(a, b, v) => {
            walk_expr_for_lambdas(a, out);
            walk_expr_for_lambdas(b, out);
            walk_expr_for_lambdas(v, out);
        }
        StatementNode::MemberAssignment(a, _, v) => {
            walk_expr_for_lambdas(a, out);
            walk_expr_for_lambdas(v, out);
        }
        StatementNode::Declaration(_, _, e, _) => walk_expr_for_lambdas(e, out),
        StatementNode::FunctionInvocation(_, _, args) => {
            for a in args {
                walk_expr_for_lambdas(a, out);
            }
        }
        StatementNode::MethodInvocation(recv, _, _, args) => {
            walk_expr_for_lambdas(recv, out);
            for a in args {
                walk_expr_for_lambdas(a, out);
            }
        }
        StatementNode::Return(Some(e)) => walk_expr_for_lambdas(e, out),
        StatementNode::Return(None) => {}
        StatementNode::IfElse(cond, then_b, elifs, else_b) => {
            walk_expr_for_lambdas(cond, out);
            walk_stmts_for_lambdas(then_b, out);
            for (c, b) in elifs {
                walk_expr_for_lambdas(c, out);
                walk_stmts_for_lambdas(b, out);
            }
            if let Some(b) = else_b {
                walk_stmts_for_lambdas(b, out);
            }
        }
        StatementNode::While(cond, body) => {
            walk_expr_for_lambdas(cond, out);
            walk_stmts_for_lambdas(body, out);
        }
        StatementNode::DoWhile(body, cond) => {
            walk_stmts_for_lambdas(body, out);
            walk_expr_for_lambdas(cond, out);
        }
        StatementNode::For(init, cond, step, body) => {
            if let Some(i) = init {
                walk_stmt_for_lambdas(i, out);
            }
            if let Some(c) = cond {
                walk_expr_for_lambdas(c, out);
            }
            if let Some(s) = step {
                walk_stmt_for_lambdas(s, out);
            }
            walk_stmts_for_lambdas(body, out);
        }
        StatementNode::Labeled(_, s) => walk_stmt_for_lambdas(s, out),
        StatementNode::Break(_) | StatementNode::Continue(_) => {}
        StatementNode::ExpressionStatement(e) => walk_expr_for_lambdas(e, out),
        StatementNode::AwaitStmt(e) => walk_expr_for_lambdas(e, out),
        StatementNode::ForEach(_, iter, _, _, body) => {
            walk_expr_for_lambdas(iter, out);
            walk_stmts_for_lambdas(body, out);
        }
        StatementNode::Switch(subj, cases, default) => {
            walk_expr_for_lambdas(subj, out);
            for (labels, body) in cases {
                for l in labels {
                    walk_expr_for_lambdas(l, out);
                }
                walk_stmts_for_lambdas(body, out);
            }
            if let Some(b) = default {
                walk_stmts_for_lambdas(b, out);
            }
        }
    }
}

fn walk_expr_for_lambdas(expr: &ExpressionNode, out: &mut HashSet<String>) {
    match expr {
        ExpressionNode::Literal(_) | ExpressionNode::Identifier(_) => {}
        ExpressionNode::ArrayLiteral(es) => {
            for e in es {
                walk_expr_for_lambdas(e, out);
            }
        }
        ExpressionNode::Binary(l, _, r) => {
            walk_expr_for_lambdas(l, out);
            walk_expr_for_lambdas(r, out);
        }
        ExpressionNode::Unary(_, e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::Parenthesized(e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::FunctionCall(_, _, args) => {
            for a in args {
                walk_expr_for_lambdas(a, out);
            }
        }
        ExpressionNode::IndexAccess(a, i) => {
            walk_expr_for_lambdas(a, out);
            walk_expr_for_lambdas(i, out);
        }
        ExpressionNode::Cast(_, e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::MemberAccess(e, _) => walk_expr_for_lambdas(e, out),
        ExpressionNode::IsExpression(e, _, _) => walk_expr_for_lambdas(e, out),
        ExpressionNode::MethodCall(recv, _, _, args) => {
            walk_expr_for_lambdas(recv, out);
            for a in args {
                walk_expr_for_lambdas(a, out);
            }
        }
        ExpressionNode::Ternary(c, t, e) => {
            walk_expr_for_lambdas(c, out);
            walk_expr_for_lambdas(t, out);
            walk_expr_for_lambdas(e, out);
        }
        ExpressionNode::Await(e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::Switch(subj, arms) => {
            walk_expr_for_lambdas(subj, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr_for_lambdas(g, out);
                }
                match &arm.body {
                    SwitchArmBody::Expr(e) => walk_expr_for_lambdas(e, out),
                    SwitchArmBody::Block(stmts) => walk_stmts_for_lambdas(stmts, out),
                }
            }
        }
        ExpressionNode::Try(e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::Lambda(l) => out.extend(lambda_free_names(l)),
        ExpressionNode::NamedArg(_, e) => walk_expr_for_lambdas(e, out),
    }
}

/// One lambda's own free names: identifiers referenced anywhere in its body — *including*,
/// transitively, inside any lambda nested within it (see the module doc comment) — that are not
/// its own parameters or a name it binds itself (`let`/`for`/`foreach`/`is ... name`/pattern
/// bindings, or a nested lambda's own parameters).
pub(in crate::semantics::analyzer) fn lambda_free_names(l: &LambdaNode) -> HashSet<String> {
    let mut bound: HashSet<String> = l.parameters.iter().map(|p| p.name.text.clone()).collect();
    let mut referenced: HashSet<String> = HashSet::new();
    match &l.body {
        LambdaBody::Expr(e) => collect_names_expr(e, &mut bound, &mut referenced),
        LambdaBody::Block(stmts) => collect_names_stmts(stmts, &mut bound, &mut referenced),
    }
    referenced.retain(|n| !bound.contains(n));
    referenced
}

fn bind_pattern(pattern: &PatternNode, bound: &mut HashSet<String>) {
    match pattern {
        PatternNode::Wildcard(_) | PatternNode::Literal(_) | PatternNode::Range(..) => {}
        PatternNode::Binding(tok) => {
            bound.insert(tok.text.clone());
        }
        PatternNode::Variant(_, _, subs) => {
            for s in subs {
                bind_pattern(s, bound);
            }
        }
        // Or-pattern alternatives are validated binding-free during analysis, so none of them
        // contribute a bound name.
        PatternNode::Or(_) => {}
    }
}

fn collect_names_stmts(
    stmts: &[StatementNode],
    bound: &mut HashSet<String>,
    referenced: &mut HashSet<String>,
) {
    for s in stmts {
        collect_names_stmt(s, bound, referenced);
    }
}

fn collect_names_stmt(
    stmt: &StatementNode,
    bound: &mut HashSet<String>,
    referenced: &mut HashSet<String>,
) {
    match stmt {
        StatementNode::Assignment(tok, e) => {
            referenced.insert(tok.text.clone());
            collect_names_expr(e, bound, referenced);
        }
        StatementNode::IndexAssignment(a, b, v) => {
            collect_names_expr(a, bound, referenced);
            collect_names_expr(b, bound, referenced);
            collect_names_expr(v, bound, referenced);
        }
        StatementNode::MemberAssignment(a, _, v) => {
            collect_names_expr(a, bound, referenced);
            collect_names_expr(v, bound, referenced);
        }
        StatementNode::Declaration(name, _, e, _) => {
            collect_names_expr(e, bound, referenced);
            bound.insert(name.text.clone());
        }
        StatementNode::FunctionInvocation(_, _, args) => {
            for a in args {
                collect_names_expr(a, bound, referenced);
            }
        }
        StatementNode::MethodInvocation(recv, _, _, args) => {
            collect_names_expr(recv, bound, referenced);
            for a in args {
                collect_names_expr(a, bound, referenced);
            }
        }
        StatementNode::Return(Some(e)) => collect_names_expr(e, bound, referenced),
        StatementNode::Return(None) => {}
        StatementNode::IfElse(cond, then_b, elifs, else_b) => {
            collect_names_expr(cond, bound, referenced);
            collect_names_stmts(then_b, bound, referenced);
            for (c, b) in elifs {
                collect_names_expr(c, bound, referenced);
                collect_names_stmts(b, bound, referenced);
            }
            if let Some(b) = else_b {
                collect_names_stmts(b, bound, referenced);
            }
        }
        StatementNode::While(cond, body) => {
            collect_names_expr(cond, bound, referenced);
            collect_names_stmts(body, bound, referenced);
        }
        StatementNode::DoWhile(body, cond) => {
            collect_names_stmts(body, bound, referenced);
            collect_names_expr(cond, bound, referenced);
        }
        StatementNode::For(init, cond, step, body) => {
            if let Some(i) = init {
                collect_names_stmt(i, bound, referenced);
            }
            if let Some(c) = cond {
                collect_names_expr(c, bound, referenced);
            }
            if let Some(s) = step {
                collect_names_stmt(s, bound, referenced);
            }
            collect_names_stmts(body, bound, referenced);
        }
        StatementNode::Labeled(_, s) => collect_names_stmt(s, bound, referenced),
        StatementNode::Break(_) | StatementNode::Continue(_) => {}
        StatementNode::ExpressionStatement(e) => collect_names_expr(e, bound, referenced),
        StatementNode::AwaitStmt(e) => collect_names_expr(e, bound, referenced),
        StatementNode::ForEach(elem, iter, idx_name, arr_name, body) => {
            collect_names_expr(iter, bound, referenced);
            bound.insert(elem.text.clone());
            bound.insert(idx_name.clone());
            bound.insert(arr_name.clone());
            collect_names_stmts(body, bound, referenced);
        }
        StatementNode::Switch(subj, cases, default) => {
            collect_names_expr(subj, bound, referenced);
            for (labels, body) in cases {
                for l in labels {
                    collect_names_expr(l, bound, referenced);
                }
                collect_names_stmts(body, bound, referenced);
            }
            if let Some(b) = default {
                collect_names_stmts(b, bound, referenced);
            }
        }
    }
}

fn collect_names_expr(
    expr: &ExpressionNode,
    bound: &mut HashSet<String>,
    referenced: &mut HashSet<String>,
) {
    match expr {
        ExpressionNode::Literal(_) => {}
        ExpressionNode::Identifier(tok) => {
            referenced.insert(tok.text.clone());
        }
        ExpressionNode::ArrayLiteral(es) => {
            for e in es {
                collect_names_expr(e, bound, referenced);
            }
        }
        ExpressionNode::Binary(l, _, r) => {
            collect_names_expr(l, bound, referenced);
            collect_names_expr(r, bound, referenced);
        }
        ExpressionNode::Unary(_, e) => collect_names_expr(e, bound, referenced),
        ExpressionNode::Parenthesized(e) => collect_names_expr(e, bound, referenced),
        ExpressionNode::FunctionCall(_, _, args) => {
            for a in args {
                collect_names_expr(a, bound, referenced);
            }
        }
        ExpressionNode::IndexAccess(a, i) => {
            collect_names_expr(a, bound, referenced);
            collect_names_expr(i, bound, referenced);
        }
        ExpressionNode::Cast(_, e) => collect_names_expr(e, bound, referenced),
        ExpressionNode::MemberAccess(e, _) => collect_names_expr(e, bound, referenced),
        ExpressionNode::IsExpression(e, _, binding) => {
            collect_names_expr(e, bound, referenced);
            if let Some(tok) = binding {
                bound.insert(tok.text.clone());
            }
        }
        ExpressionNode::MethodCall(recv, _, _, args) => {
            collect_names_expr(recv, bound, referenced);
            for a in args {
                collect_names_expr(a, bound, referenced);
            }
        }
        ExpressionNode::Ternary(c, t, e) => {
            collect_names_expr(c, bound, referenced);
            collect_names_expr(t, bound, referenced);
            collect_names_expr(e, bound, referenced);
        }
        ExpressionNode::Await(e) => collect_names_expr(e, bound, referenced),
        ExpressionNode::Switch(subj, arms) => {
            collect_names_expr(subj, bound, referenced);
            for arm in arms {
                bind_pattern(&arm.pattern, bound);
                if let Some(g) = &arm.guard {
                    collect_names_expr(g, bound, referenced);
                }
                match &arm.body {
                    SwitchArmBody::Expr(e) => collect_names_expr(e, bound, referenced),
                    SwitchArmBody::Block(stmts) => collect_names_stmts(stmts, bound, referenced),
                }
            }
        }
        ExpressionNode::Try(e) => collect_names_expr(e, bound, referenced),
        // Transitive capture (multi-level): descend into a nested lambda's own body too, so a
        // grandparent's local referenced only inside a doubly-nested lambda still shows up as one
        // of *this* lambda's free names — which is exactly what makes `analyze_lambda` forward it
        // as one of this lambda's own captures, so the inner lambda can in turn receive it from
        // this one. The nested lambda's own parameters are folded into the shared `bound` set first
        // (same flat/over-approximate treatment as every other binder here — see the module doc
        // comment), so they are never mistaken for a name this lambda itself needs to capture.
        ExpressionNode::Lambda(l) => {
            for p in &l.parameters {
                bound.insert(p.name.text.clone());
            }
            match &l.body {
                LambdaBody::Expr(e) => collect_names_expr(e, bound, referenced),
                LambdaBody::Block(stmts) => collect_names_stmts(stmts, bound, referenced),
            }
        }
        ExpressionNode::NamedArg(_, e) => collect_names_expr(e, bound, referenced),
    }
}
