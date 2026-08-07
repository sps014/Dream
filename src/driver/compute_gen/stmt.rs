//! Statement → WGSL lowering.

use super::context::EmitCtx;
use super::expr::{coerce_expr_to_wgsl_ty, emit_call, emit_expr};
use super::ty::{dream_ty_to_wgsl, infer_wgsl_ty};
use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::expression::ExpressionNode;
use dream_syntax::nodes::statement::StatementNode;
use dream_text::text_span::TextSpan;

fn stmt_span(stmt: &StatementNode<'_>) -> Option<TextSpan> {
    match stmt {
        StatementNode::Assignment(tok, _)
        | StatementNode::Declaration(tok, _, _, _)
        | StatementNode::WorkgroupDecl(tok, _, _)
        | StatementNode::FunctionInvocation(tok, _, _)
        | StatementNode::MethodInvocation(_, tok, _, _)
        | StatementNode::MemberAssignment(_, tok, _)
        | StatementNode::ForEach(tok, _, _, _, _) => Some(tok.position),
        StatementNode::TupleDeclaration { names, init, .. } => names
            .first()
            .map(|n| n.position)
            .or_else(|| init.position()),
        StatementNode::IndexAssignment(arr, _, _) => arr.position(),
        StatementNode::Return(Some(e))
        | StatementNode::ExpressionStatement(e)
        | StatementNode::AwaitStmt(e)
        | StatementNode::While(e, _)
        | StatementNode::DoWhile(_, e)
        | StatementNode::Lock(e, _)
        | StatementNode::IfElse(e, _, _, _)
        | StatementNode::Switch(e, _, _) => e.position(),
        StatementNode::For(_, Some(cond), _, _) => cond.position(),
        StatementNode::Labeled(_, inner) => stmt_span(inner),
        StatementNode::Return(None)
        | StatementNode::For(_, None, _, _)
        | StatementNode::Break(_)
        | StatementNode::Continue(_) => None,
    }
}

pub(super) fn emit_stmts(
    stmts: &[StatementNode<'_>],
    out: &mut String,
    wg: &mut String,
    indent: usize,
    ctx: &EmitCtx<'_>,
    diagnostics: &mut DiagnosticBag,
    kernel: &str,
) {
    ctx.push_scope();
    for s in stmts {
        emit_stmt(s, out, wg, indent, ctx, diagnostics, kernel);
    }
    ctx.pop_scope();
}

fn pad(n: usize) -> String {
    "  ".repeat(n)
}

fn emit_stmt(
    stmt: &StatementNode<'_>,
    out: &mut String,
    wg: &mut String,
    indent: usize,
    ctx: &EmitCtx<'_>,
    diagnostics: &mut DiagnosticBag,
    kernel: &str,
) {
    let p = pad(indent);
    match stmt {
        StatementNode::WorkgroupDecl(name, ty, size) => {
            let elem = dream_ty_to_wgsl(ty);
            wg.push_str(&format!(
                "var<workgroup> {}: array<{}, {}>;\n",
                ctx.mangle(&name.text),
                elem,
                size
            ));
        }
        StatementNode::Declaration(name, ty, init, _) => {
            if let ExpressionNode::IncDec {
                prefix,
                is_inc,
                target,
                ..
            } = init
            {
                let place = emit_expr(target, ctx);
                let op = if *is_inc { "+" } else { "-" };
                let t = ty
                    .as_ref()
                    .map(dream_ty_to_wgsl)
                    .unwrap_or_else(|| infer_wgsl_ty(init, ctx));
                ctx.define_local(&name.text, t.clone());
                if *prefix {
                    out.push_str(&format!("{}{} = {} {} 1;\n", p, place, place, op));
                    out.push_str(&format!("{}var {}: {} = {};\n", p, name.text, t, place));
                } else {
                    out.push_str(&format!("{}var {}: {} = {};\n", p, name.text, t, place));
                    out.push_str(&format!("{}{} = {} {} 1;\n", p, place, place, op));
                }
                return;
            }
            let t = ty
                .as_ref()
                .map(dream_ty_to_wgsl)
                .unwrap_or_else(|| infer_wgsl_ty(init, ctx));
            let init_s = coerce_expr_to_wgsl_ty(init, &t, ctx);
            ctx.define_local(&name.text, t.clone());
            out.push_str(&format!(
                "{}var {}: {} = {};\n",
                p,
                name.text,
                t,
                init_s
            ));
        }
        StatementNode::Assignment(name, value) => {
            let lhs = ctx.rewrite_ident(&name.text);
            let want = ctx
                .lookup_local(&name.text)
                .unwrap_or_else(|| infer_wgsl_ty(value, ctx));
            let rhs = coerce_expr_to_wgsl_ty(value, &want, ctx);
            out.push_str(&format!("{}{} = {};\n", p, lhs, rhs));
        }
        StatementNode::IndexAssignment(arr, idx, value) => {
            let arr_s = emit_expr(arr, ctx);
            let idx_s = coerce_expr_to_wgsl_ty(idx, "i32", ctx);
            let elem_ty = match arr {
                ExpressionNode::Identifier(name) => {
                    if let Some(t) = ctx.lookup_local(&name.text) {
                        t.strip_prefix("array<")
                            .and_then(|s| s.strip_suffix('>'))
                            .unwrap_or(t.as_str())
                            .to_string()
                    } else if let Some(b) = ctx.binding(&name.text) {
                        b.wgsl_ty.clone()
                    } else {
                        infer_wgsl_ty(value, ctx)
                    }
                }
                _ => infer_wgsl_ty(value, ctx),
            };
            let val_s = coerce_expr_to_wgsl_ty(value, &elem_ty, ctx);
            let atomic = matches!(arr, ExpressionNode::Identifier(n) if ctx.is_atomic_buf(&n.text));
            if atomic {
                out.push_str(&format!(
                    "{}atomicStore(&{}[u32({})], {});\n",
                    p, arr_s, idx_s, val_s
                ));
            } else {
                out.push_str(&format!("{}{}[u32({})] = {};\n", p, arr_s, idx_s, val_s));
            }
        }
        StatementNode::MemberAssignment(obj, member, value) => {
            out.push_str(&format!(
                "{}.{} = {};\n",
                emit_expr(obj, ctx),
                member.text,
                emit_expr(value, ctx)
            ));
        }
        StatementNode::Return(None) => out.push_str(&format!("{}return;\n", p)),
        StatementNode::Return(Some(_)) => out.push_str(&format!("{}return;\n", p)),
        StatementNode::IfElse(cond, then_b, elifs, else_b) => {
            out.push_str(&format!("{}if ({}) {{\n", p, emit_expr(cond, ctx)));
            emit_stmts(then_b, out, wg, indent + 1, ctx, diagnostics, kernel);
            out.push_str(&format!("{}}}\n", p));
            for (c, body) in elifs {
                out.push_str(&format!("{}else if ({}) {{\n", p, emit_expr(c, ctx)));
                emit_stmts(body, out, wg, indent + 1, ctx, diagnostics, kernel);
                out.push_str(&format!("{}}}\n", p));
            }
            if let Some(eb) = else_b {
                out.push_str(&format!("{}else {{\n", p));
                emit_stmts(eb, out, wg, indent + 1, ctx, diagnostics, kernel);
                out.push_str(&format!("{}}}\n", p));
            }
        }
        StatementNode::While(cond, body) => {
            out.push_str(&format!("{}loop {{\n", p));
            out.push_str(&format!(
                "{}  if (!({})) {{ break; }}\n",
                p,
                emit_expr(cond, ctx)
            ));
            emit_stmts(body, out, wg, indent + 1, ctx, diagnostics, kernel);
            out.push_str(&format!("{}}}\n", p));
        }
        StatementNode::DoWhile(body, cond) => {
            out.push_str(&format!("{}loop {{\n", p));
            emit_stmts(body, out, wg, indent + 1, ctx, diagnostics, kernel);
            out.push_str(&format!(
                "{}  if (!({})) {{ break; }}\n",
                p,
                emit_expr(cond, ctx)
            ));
            out.push_str(&format!("{}}}\n", p));
        }
        StatementNode::For(init, cond, step, body) => {
            if let Some(i) = init {
                emit_stmt(i, out, wg, indent, ctx, diagnostics, kernel);
            }
            out.push_str(&format!("{}loop {{\n", p));
            if let Some(c) = cond {
                out.push_str(&format!(
                    "{}  if (!({})) {{ break; }}\n",
                    p,
                    emit_expr(c, ctx)
                ));
            }
            emit_stmts(body, out, wg, indent + 1, ctx, diagnostics, kernel);
            if let Some(s) = step {
                emit_stmt(s, out, wg, indent + 1, ctx, diagnostics, kernel);
            }
            out.push_str(&format!("{}}}\n", p));
        }
        StatementNode::Break(_) => out.push_str(&format!("{}break;\n", p)),
        StatementNode::Continue(_) => out.push_str(&format!("{}continue;\n", p)),
        StatementNode::Labeled(_, inner) => emit_stmt(inner, out, wg, indent, ctx, diagnostics, kernel),
        StatementNode::Switch(subject, cases, default) => {
            // Lower to if-else chain (WGSL switch is more limited).
            let sub = emit_expr(subject, ctx);
            let mut first = true;
            for (labels, body) in cases {
                let conds: Vec<String> = labels
                    .iter()
                    .map(|l| format!("({}) == ({})", sub, emit_expr(l, ctx)))
                    .collect();
                let kw = if first { "if" } else { "else if" };
                first = false;
                out.push_str(&format!("{}{} ({}) {{\n", p, kw, conds.join(" || ")));
                emit_stmts(body, out, wg, indent + 1, ctx, diagnostics, kernel);
                out.push_str(&format!("{}}}\n", p));
            }
            if let Some(db) = default {
                out.push_str(&format!("{}else {{\n", p));
                emit_stmts(db, out, wg, indent + 1, ctx, diagnostics, kernel);
                out.push_str(&format!("{}}}\n", p));
            }
        }
        StatementNode::FunctionInvocation(name, _, args)
        | StatementNode::MethodInvocation(_, name, _, args) => {
            let call = emit_call(&name.text, args, ctx);
            out.push_str(&format!("{}{};\n", p, call));
        }
        StatementNode::ExpressionStatement(e) => {
            out.push_str(&format!("{}{};\n", p, emit_expr(e, ctx)));
        }
        StatementNode::ForEach(..)
        | StatementNode::AwaitStmt(_)
        | StatementNode::Lock(..)
        | StatementNode::TupleDeclaration { .. } => {
            let kind = match stmt {
                StatementNode::ForEach(..) => "for-each",
                StatementNode::AwaitStmt(_) => "await",
                StatementNode::Lock(..) => "lock",
                StatementNode::TupleDeclaration { .. } => "tuple declaration",
                _ => "statement",
            };
            diagnostics.report_error(
                format!(
                    "@compute kernel '{kernel}' contains unsupported {kind}; remove it or rewrite with supported control flow"
                ),
                stmt_span(stmt),
            );
        }
    }
}
