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
//!   analyzed (so the enclosing `let`s it finds can be boxed into `CaptureCell<T>` before they are
//!   emitted — see `Analyzer::boxed_locals`). Finds every lambda anywhere in the body (any nesting
//!   depth) and unions each one's own free names.
//! - [`lambda_free_names`] — one lambda's own free names (referenced but not bound within its own
//!   body), used both by the pre-pass above and again when the lambda itself is analyzed (to build
//!   its capture list / synthesized environment — see `expressions::lambda`).
//!
//! Free-name collection uses a **scope stack**: binders cover only their region (block / arm /
//! foreach / nested-lambda params). A use of `x` before a later `let x` still sees the outer `x`.
//! Nested lambdas get their own binder scopes and do not pollute the parent's bound set.

use dream_syntax::nodes::{
    ExpressionNode, LambdaBody, LambdaNode, PatternNode, StatementNode, SwitchArmBody,
};
use std::collections::HashSet;

/// Unions [`lambda_free_names`] over every arrow-lambda anywhere in `stmts` (a whole function
/// body), at any statement/expression nesting depth. Does not descend *into* a found lambda's own
/// body beyond computing its own free names — a lambda nested within it is that lambda's own
/// separate concern (see the module doc comment's capture-scope note), found and scanned
/// separately once that lambda's own turn to be analyzed comes up.
pub(in crate::analyzer) fn scan_function_captures(
    stmts: &[StatementNode],
) -> HashSet<String> {
    let mut out = HashSet::new();
    walk_stmts_for_lambdas(stmts, &mut out);
    out
}

/// Every name anywhere in `stmts` (a whole function body, same traversal shape as
/// [`scan_function_captures`]) that is ever passed as a `ref` argument to a local/parameter place
/// (`f(ref x)`). Run as a pre-pass alongside `scan_function_captures` (feeding
/// `Analyzer::ref_boxed_locals`, minus whatever `scan_function_captures` already claims for
/// `Analyzer::boxed_locals`) so `x`'s slot is boxed (into `RefBox<T>`, or `CaptureCell<T>` if `x` is
/// also captured) from its very first `let`/parameter binding — a `ref` argument always needs the
/// shared box pointer, exactly like a
/// closure capture does, and the two triggers simply union into one boxing set. Over-approximates
/// like its sibling: a name is collected regardless of whether the call it's passed to turns out
/// to actually resolve to a `ref` parameter (a later mismatch is a separate diagnostic, not this
/// pass's concern) — boxing a name that wasn't strictly needed is never unsound, only unnecessary.
pub(in crate::analyzer) fn scan_ref_argument_targets(
    stmts: &[StatementNode],
) -> HashSet<String> {
    let mut out = HashSet::new();
    walk_stmts_for_ref_targets(stmts, &mut out);
    out
}

fn walk_stmts_for_ref_targets(stmts: &[StatementNode], out: &mut HashSet<String>) {
    for s in stmts {
        walk_stmt_for_ref_targets(s, out);
    }
}

fn walk_stmt_for_ref_targets(stmt: &StatementNode, out: &mut HashSet<String>) {
    match stmt {
        StatementNode::Assignment(_, e) => walk_expr_for_ref_targets(e, out),
        StatementNode::IndexAssignment(a, b, v) => {
            walk_expr_for_ref_targets(a, out);
            walk_expr_for_ref_targets(b, out);
            walk_expr_for_ref_targets(v, out);
        }
        StatementNode::MemberAssignment(a, _, v) => {
            walk_expr_for_ref_targets(a, out);
            walk_expr_for_ref_targets(v, out);
        }
        StatementNode::Declaration(_, _, e, _)
        | StatementNode::TupleDeclaration { init: e, .. } => walk_expr_for_ref_targets(e, out),
        StatementNode::FunctionInvocation(_, _, args) => {
            for a in args {
                walk_expr_for_ref_targets(a, out);
            }
        }
        StatementNode::MethodInvocation(recv, _, _, args) => {
            walk_expr_for_ref_targets(recv, out);
            for a in args {
                walk_expr_for_ref_targets(a, out);
            }
        }
        StatementNode::Return(Some(e)) => walk_expr_for_ref_targets(e, out),
        StatementNode::Return(None) => {}
        StatementNode::IfElse(cond, then_b, elifs, else_b) => {
            walk_expr_for_ref_targets(cond, out);
            walk_stmts_for_ref_targets(then_b, out);
            for (c, b) in elifs {
                walk_expr_for_ref_targets(c, out);
                walk_stmts_for_ref_targets(b, out);
            }
            if let Some(b) = else_b {
                walk_stmts_for_ref_targets(b, out);
            }
        }
        StatementNode::While(cond, body) => {
            walk_expr_for_ref_targets(cond, out);
            walk_stmts_for_ref_targets(body, out);
        }
        StatementNode::DoWhile(body, cond) => {
            walk_stmts_for_ref_targets(body, out);
            walk_expr_for_ref_targets(cond, out);
        }
        StatementNode::Lock(target, body) => {
            walk_expr_for_ref_targets(target, out);
            walk_stmts_for_ref_targets(body, out);
        }
        StatementNode::For(init, cond, step, body) => {
            if let Some(i) = init {
                walk_stmt_for_ref_targets(i, out);
            }
            if let Some(c) = cond {
                walk_expr_for_ref_targets(c, out);
            }
            if let Some(s) = step {
                walk_stmt_for_ref_targets(s, out);
            }
            walk_stmts_for_ref_targets(body, out);
        }
        StatementNode::Labeled(_, s) => walk_stmt_for_ref_targets(s, out),
        StatementNode::Break(_) | StatementNode::Continue(_) => {}
        StatementNode::ExpressionStatement(e) => walk_expr_for_ref_targets(e, out),
        StatementNode::AwaitStmt(e) => walk_expr_for_ref_targets(e, out),
        StatementNode::ForEach(_, iter, _, _, body) => {
            walk_expr_for_ref_targets(iter, out);
            walk_stmts_for_ref_targets(body, out);
        }
        StatementNode::Switch(subj, cases, default) => {
            walk_expr_for_ref_targets(subj, out);
            for (labels, body) in cases {
                for l in labels {
                    walk_expr_for_ref_targets(l, out);
                }
                walk_stmts_for_ref_targets(body, out);
            }
            if let Some(b) = default {
                walk_stmts_for_ref_targets(b, out);
            }
        }
        StatementNode::WorkgroupDecl(_, _, _) => {}
    }
}

fn walk_expr_for_ref_targets(expr: &ExpressionNode, out: &mut HashSet<String>) {
    match expr {
        ExpressionNode::Literal(_) | ExpressionNode::Identifier(_) => {}
        ExpressionNode::RefArgument(inner) => {
            if let ExpressionNode::Identifier(tok) = inner {
                out.insert(tok.text.clone());
            }
            walk_expr_for_ref_targets(inner, out);
        }
        ExpressionNode::ArrayLiteral(es)
        | ExpressionNode::SetLiteral(es)
        | ExpressionNode::TupleLiteral(es) => {
            for e in es {
                walk_expr_for_ref_targets(e, out);
            }
        }
        ExpressionNode::MapLiteral(entries) => {
            for (k, v) in entries {
                walk_expr_for_ref_targets(k, out);
                walk_expr_for_ref_targets(v, out);
            }
        }
        ExpressionNode::Binary(l, _, r) => {
            walk_expr_for_ref_targets(l, out);
            walk_expr_for_ref_targets(r, out);
        }
        ExpressionNode::Unary(_, e) => walk_expr_for_ref_targets(e, out),
        ExpressionNode::IncDec { target, .. } => walk_expr_for_ref_targets(target, out),
        ExpressionNode::Parenthesized(_, e) => walk_expr_for_ref_targets(e, out),
        ExpressionNode::FunctionCall(_, _, args) => {
            for a in args {
                walk_expr_for_ref_targets(a, out);
            }
        }
        ExpressionNode::Call(callee, _, args) => {
            walk_expr_for_ref_targets(callee, out);
            for a in args {
                walk_expr_for_ref_targets(a, out);
            }
        }
        ExpressionNode::IndexAccess(a, i) => {
            walk_expr_for_ref_targets(a, out);
            walk_expr_for_ref_targets(i, out);
        }
        ExpressionNode::Cast(_, e) => walk_expr_for_ref_targets(e, out),
        ExpressionNode::MemberAccess(e, _) => walk_expr_for_ref_targets(e, out),
        ExpressionNode::IsExpression(e, _, _) => walk_expr_for_ref_targets(e, out),
        ExpressionNode::MethodCall(recv, _, _, args) => {
            walk_expr_for_ref_targets(recv, out);
            for a in args {
                walk_expr_for_ref_targets(a, out);
            }
        }
        ExpressionNode::Ternary(c, t, e) => {
            walk_expr_for_ref_targets(c, out);
            walk_expr_for_ref_targets(t, out);
            walk_expr_for_ref_targets(e, out);
        }
        ExpressionNode::Await(_, e) => walk_expr_for_ref_targets(e, out),
        ExpressionNode::Switch(subj, arms) => {
            walk_expr_for_ref_targets(subj, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr_for_ref_targets(g, out);
                }
                match &arm.body {
                    SwitchArmBody::Expr(e) => walk_expr_for_ref_targets(e, out),
                    SwitchArmBody::Block(stmts) => walk_stmts_for_ref_targets(stmts, out),
                }
            }
        }
        ExpressionNode::Try(e) => walk_expr_for_ref_targets(e, out),
        // A lambda's own ref-argument uses are that lambda's own concern (scanned again when its
        // own turn to be analyzed comes up, exactly like `walk_expr_for_lambdas` treats a nested
        // lambda's captures) — except a ref target that is itself one of *this* function's names,
        // reached through the lambda body, still needs to be found: descend the same as
        // `collect_names_expr` does for captures.
        ExpressionNode::Lambda(l) => match &l.body {
            LambdaBody::Expr(e) => walk_expr_for_ref_targets(e, out),
            LambdaBody::Block(stmts) => walk_stmts_for_ref_targets(stmts, out),
        },
        ExpressionNode::NamedArg(_, e) => walk_expr_for_ref_targets(e, out),
        ExpressionNode::SyntaxBlock(block) => {
            for part in &block.parts {
                if let dream_syntax::nodes::SyntaxBlockPart::Splice(e) = part {
                    walk_expr_for_ref_targets(e, out);
                }
            }
        }
    }
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
        StatementNode::Declaration(_, _, e, _)
        | StatementNode::TupleDeclaration { init: e, .. } => walk_expr_for_lambdas(e, out),
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
        StatementNode::Lock(target, body) => {
            walk_expr_for_lambdas(target, out);
            walk_stmts_for_lambdas(body, out);
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
        StatementNode::WorkgroupDecl(_, _, _) => {}
    }
}

fn walk_expr_for_lambdas(expr: &ExpressionNode, out: &mut HashSet<String>) {
    match expr {
        ExpressionNode::Literal(_) | ExpressionNode::Identifier(_) => {}
        ExpressionNode::ArrayLiteral(es)
        | ExpressionNode::SetLiteral(es)
        | ExpressionNode::TupleLiteral(es) => {
            for e in es {
                walk_expr_for_lambdas(e, out);
            }
        }
        ExpressionNode::MapLiteral(entries) => {
            for (k, v) in entries {
                walk_expr_for_lambdas(k, out);
                walk_expr_for_lambdas(v, out);
            }
        }
        ExpressionNode::Binary(l, _, r) => {
            walk_expr_for_lambdas(l, out);
            walk_expr_for_lambdas(r, out);
        }
        ExpressionNode::Unary(_, e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::IncDec { target, .. } => walk_expr_for_lambdas(target, out),
        ExpressionNode::Parenthesized(_, e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::FunctionCall(_, _, args) => {
            for a in args {
                walk_expr_for_lambdas(a, out);
            }
        }
        ExpressionNode::Call(callee, _, args) => {
            walk_expr_for_lambdas(callee, out);
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
        ExpressionNode::Await(_, e) => walk_expr_for_lambdas(e, out),
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
        ExpressionNode::RefArgument(e) => walk_expr_for_lambdas(e, out),
        ExpressionNode::SyntaxBlock(block) => {
            for part in &block.parts {
                if let dream_syntax::nodes::SyntaxBlockPart::Splice(e) = part {
                    walk_expr_for_lambdas(e, out);
                }
            }
        }
    }
}

/// One lambda's own free names: identifiers referenced in its body that are not bound by a
/// covering scope inside the lambda (params, `let`s after their declaration, foreach binders,
/// switch-arm pattern bindings, nested-lambda params). Nested lambdas contribute their own free
/// names transitively (filtered against this lambda's scopes).
pub(in crate::analyzer) fn lambda_free_names(l: &LambdaNode) -> HashSet<String> {
    let mut scopes: Vec<HashSet<String>> = vec![l
        .parameters
        .iter()
        .map(|p| p.name.text.clone())
        .collect()];
    let mut referenced: HashSet<String> = HashSet::new();
    match &l.body {
        LambdaBody::Expr(e) => collect_names_expr(e, &mut scopes, &mut referenced),
        LambdaBody::Block(stmts) => collect_names_stmts(stmts, &mut scopes, &mut referenced),
    }
    referenced
}

fn is_bound(scopes: &[HashSet<String>], name: &str) -> bool {
    scopes.iter().rev().any(|s| s.contains(name))
}

fn bind_here(scopes: &mut [HashSet<String>], name: String) {
    if let Some(top) = scopes.last_mut() {
        top.insert(name);
    }
}

fn push_scope(scopes: &mut Vec<HashSet<String>>) {
    scopes.push(HashSet::new());
}

fn pop_scope(scopes: &mut Vec<HashSet<String>>) {
    scopes.pop();
}

fn bind_pattern(pattern: &PatternNode, scopes: &mut [HashSet<String>]) {
    match pattern {
        PatternNode::Wildcard(_) | PatternNode::Literal(_) | PatternNode::Range(..) => {}
        PatternNode::Binding(tok) => bind_here(scopes, tok.text.clone()),
        PatternNode::Variant(_, _, subs) => {
            for s in subs {
                bind_pattern(s, scopes);
            }
        }
        PatternNode::Or(_) => {}
    }
}

fn collect_names_stmts(
    stmts: &[StatementNode],
    scopes: &mut Vec<HashSet<String>>,
    referenced: &mut HashSet<String>,
) {
    for s in stmts {
        collect_names_stmt(s, scopes, referenced);
    }
}

fn collect_names_block(
    stmts: &[StatementNode],
    scopes: &mut Vec<HashSet<String>>,
    referenced: &mut HashSet<String>,
) {
    push_scope(scopes);
    collect_names_stmts(stmts, scopes, referenced);
    pop_scope(scopes);
}

fn collect_names_stmt(
    stmt: &StatementNode,
    scopes: &mut Vec<HashSet<String>>,
    referenced: &mut HashSet<String>,
) {
    match stmt {
        StatementNode::Assignment(tok, e) => {
            if !is_bound(scopes, &tok.text) {
                referenced.insert(tok.text.clone());
            }
            collect_names_expr(e, scopes, referenced);
        }
        StatementNode::IndexAssignment(a, b, v) => {
            collect_names_expr(a, scopes, referenced);
            collect_names_expr(b, scopes, referenced);
            collect_names_expr(v, scopes, referenced);
        }
        StatementNode::MemberAssignment(a, _, v) => {
            collect_names_expr(a, scopes, referenced);
            collect_names_expr(v, scopes, referenced);
        }
        StatementNode::Declaration(name, _, e, _) => {
            // Init sees outer scopes; the name binds only after the initializer.
            collect_names_expr(e, scopes, referenced);
            bind_here(scopes, name.text.clone());
        }
        StatementNode::TupleDeclaration { names, init, .. } => {
            collect_names_expr(init, scopes, referenced);
            for name in names {
                bind_here(scopes, name.text.clone());
            }
        }
        StatementNode::FunctionInvocation(name, _, args) => {
            if !is_bound(scopes, &name.text) {
                referenced.insert(name.text.clone());
            }
            for a in args {
                collect_names_expr(a, scopes, referenced);
            }
        }
        StatementNode::MethodInvocation(recv, _, _, args) => {
            collect_names_expr(recv, scopes, referenced);
            for a in args {
                collect_names_expr(a, scopes, referenced);
            }
        }
        StatementNode::Return(Some(e)) => collect_names_expr(e, scopes, referenced),
        StatementNode::Return(None) => {}
        StatementNode::IfElse(cond, then_b, elifs, else_b) => {
            collect_names_expr(cond, scopes, referenced);
            collect_names_block(then_b, scopes, referenced);
            for (c, b) in elifs {
                collect_names_expr(c, scopes, referenced);
                collect_names_block(b, scopes, referenced);
            }
            if let Some(b) = else_b {
                collect_names_block(b, scopes, referenced);
            }
        }
        StatementNode::While(cond, body) => {
            collect_names_expr(cond, scopes, referenced);
            collect_names_block(body, scopes, referenced);
        }
        StatementNode::DoWhile(body, cond) => {
            collect_names_block(body, scopes, referenced);
            collect_names_expr(cond, scopes, referenced);
        }
        StatementNode::Lock(target, body) => {
            collect_names_expr(target, scopes, referenced);
            collect_names_block(body, scopes, referenced);
        }
        StatementNode::For(init, cond, step, body) => {
            push_scope(scopes);
            if let Some(i) = init {
                collect_names_stmt(i, scopes, referenced);
            }
            if let Some(c) = cond {
                collect_names_expr(c, scopes, referenced);
            }
            if let Some(s) = step {
                collect_names_stmt(s, scopes, referenced);
            }
            collect_names_block(body, scopes, referenced);
            pop_scope(scopes);
        }
        StatementNode::Labeled(_, s) => collect_names_stmt(s, scopes, referenced),
        StatementNode::Break(_) | StatementNode::Continue(_) => {}
        StatementNode::ExpressionStatement(e) => collect_names_expr(e, scopes, referenced),
        StatementNode::AwaitStmt(e) => collect_names_expr(e, scopes, referenced),
        StatementNode::ForEach(elem, iter, idx_name, arr_name, body) => {
            collect_names_expr(iter, scopes, referenced);
            push_scope(scopes);
            bind_here(scopes, elem.text.clone());
            bind_here(scopes, idx_name.clone());
            bind_here(scopes, arr_name.clone());
            collect_names_stmts(body, scopes, referenced);
            pop_scope(scopes);
        }
        StatementNode::Switch(subj, cases, default) => {
            collect_names_expr(subj, scopes, referenced);
            for (labels, body) in cases {
                for l in labels {
                    collect_names_expr(l, scopes, referenced);
                }
                collect_names_block(body, scopes, referenced);
            }
            if let Some(b) = default {
                collect_names_block(b, scopes, referenced);
            }
        }
        StatementNode::WorkgroupDecl(_, _, _) => {}
    }
}

fn collect_names_expr(
    expr: &ExpressionNode,
    scopes: &mut Vec<HashSet<String>>,
    referenced: &mut HashSet<String>,
) {
    match expr {
        ExpressionNode::Literal(_) => {}
        ExpressionNode::Identifier(tok) => {
            if !is_bound(scopes, &tok.text) {
                referenced.insert(tok.text.clone());
            }
        }
        ExpressionNode::ArrayLiteral(es)
        | ExpressionNode::SetLiteral(es)
        | ExpressionNode::TupleLiteral(es) => {
            for e in es {
                collect_names_expr(e, scopes, referenced);
            }
        }
        ExpressionNode::MapLiteral(entries) => {
            for (k, v) in entries {
                collect_names_expr(k, scopes, referenced);
                collect_names_expr(v, scopes, referenced);
            }
        }
        ExpressionNode::Binary(l, _, r) => {
            collect_names_expr(l, scopes, referenced);
            collect_names_expr(r, scopes, referenced);
        }
        ExpressionNode::Unary(_, e) => collect_names_expr(e, scopes, referenced),
        ExpressionNode::IncDec { target, .. } => collect_names_expr(target, scopes, referenced),
        ExpressionNode::Parenthesized(_, e) => collect_names_expr(e, scopes, referenced),
        ExpressionNode::FunctionCall(name, _, args) => {
            if !is_bound(scopes, &name.text) {
                referenced.insert(name.text.clone());
            }
            for a in args {
                collect_names_expr(a, scopes, referenced);
            }
        }
        ExpressionNode::Call(callee, _, args) => {
            collect_names_expr(callee, scopes, referenced);
            for a in args {
                collect_names_expr(a, scopes, referenced);
            }
        }
        ExpressionNode::IndexAccess(a, i) => {
            collect_names_expr(a, scopes, referenced);
            collect_names_expr(i, scopes, referenced);
        }
        ExpressionNode::Cast(_, e) => collect_names_expr(e, scopes, referenced),
        ExpressionNode::MemberAccess(e, _) => collect_names_expr(e, scopes, referenced),
        // `is`-with-binding is scoped by the statement layer (`if`/`while`); here the binding is
        // only recorded when analyzing those statements' branches, so ignore it on the expression.
        ExpressionNode::IsExpression(e, _, _) => collect_names_expr(e, scopes, referenced),
        ExpressionNode::MethodCall(recv, _, _, args) => {
            collect_names_expr(recv, scopes, referenced);
            for a in args {
                collect_names_expr(a, scopes, referenced);
            }
        }
        ExpressionNode::Ternary(c, t, e) => {
            collect_names_expr(c, scopes, referenced);
            collect_names_expr(t, scopes, referenced);
            collect_names_expr(e, scopes, referenced);
        }
        ExpressionNode::Await(_, e) => collect_names_expr(e, scopes, referenced),
        ExpressionNode::Switch(subj, arms) => {
            collect_names_expr(subj, scopes, referenced);
            for arm in arms {
                push_scope(scopes);
                bind_pattern(&arm.pattern, scopes);
                if let Some(g) = &arm.guard {
                    collect_names_expr(g, scopes, referenced);
                }
                match &arm.body {
                    SwitchArmBody::Expr(e) => collect_names_expr(e, scopes, referenced),
                    SwitchArmBody::Block(stmts) => collect_names_stmts(stmts, scopes, referenced),
                }
                pop_scope(scopes);
            }
        }
        ExpressionNode::Try(e) => collect_names_expr(e, scopes, referenced),
        // Transitive capture: a nested lambda's free names that aren't bound here are free here too.
        ExpressionNode::Lambda(l) => {
            for free in lambda_free_names(l) {
                if !is_bound(scopes, &free) {
                    referenced.insert(free);
                }
            }
        }
        ExpressionNode::NamedArg(_, e) => collect_names_expr(e, scopes, referenced),
        ExpressionNode::RefArgument(e) => collect_names_expr(e, scopes, referenced),
        ExpressionNode::SyntaxBlock(block) => {
            for part in &block.parts {
                if let dream_syntax::nodes::SyntaxBlockPart::Splice(e) = part {
                    collect_names_expr(e, scopes, referenced);
                }
            }
        }
    }
}
