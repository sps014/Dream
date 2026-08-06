//! In-arena rewrite of `SyntaxBlock` expressions into ordinary Dream expressions.

use bumpalo::Bump;
use dream_diagnostics::DiagnosticBag;
use dream_syntax::lexer::Lexer;
use dream_syntax::nodes::{
    ExpressionNode, LambdaBody, StatementNode, SwitchArm, SwitchArmBody, SyntaxBlockPart,
};
use dream_syntax::parser::Parser;
use dream_syntax::nodes::function::FunctionNode;
use indexmap::IndexMap;
use std::io::Error;

/// Parses a Dream expression from `source` and returns it arena-allocated.
pub fn parse_expression_source<'a>(
    arena: &'a Bump,
    source: &str,
    diagnostics: &mut DiagnosticBag,
) -> Result<ExpressionNode<'a>, Error> {
    // Wrap in `fun __g(): void { let __x = <expr>; }` then extract — simpler: parse as
    // `fun __gen(): void { return <expr>; }` and pull the return expression.
    let wrapped = format!("fun __gen(): void {{ return {}; }}\n", source);
    let mut local = DiagnosticBag::new(Some("<syntax-replace>".into()));
    let lexer = Lexer::new(wrapped);
    let mut parser = Parser::new(lexer, arena, &mut local);
    let ast = match parser.parse() {
        Ok(a) => a,
        Err(e) => {
            diagnostics.extend(&local);
            return Err(e);
        }
    };
    diagnostics.extend(&local);
    let program = ast.get_root();
    let f = program
        .functions
        .first()
        .ok_or_else(|| Error::new(std::io::ErrorKind::InvalidData, "replace parse produced no function"))?;
    for stmt in f.body {
        if let StatementNode::Return(Some(e)) = stmt {
            return Ok(e.clone());
        }
    }
    Err(Error::new(
        std::io::ErrorKind::InvalidData,
        "replace parse: expected return expression",
    ))
}

/// Rebuilds a function body, replacing syntax blocks whose site key is in `by_site`.
pub fn rewrite_function_body<'a>(
    arena: &'a Bump,
    body: &'a [StatementNode<'a>],
    by_site: &IndexMap<(String, String), String>,
    diagnostics: &mut DiagnosticBag,
) -> Result<&'a [StatementNode<'a>], Error> {
    let mut changed = false;
    let mut out = Vec::with_capacity(body.len());
    for s in body {
        out.push(rewrite_stmt(arena, s, by_site, diagnostics, &mut changed)?);
    }
    if changed {
        Ok(arena.alloc_slice_fill_iter(out))
    } else {
        Ok(body)
    }
}

pub fn rewrite_function<'a>(
    arena: &'a Bump,
    f: &mut FunctionNode<'a>,
    by_site: &IndexMap<(String, String), String>,
    diagnostics: &mut DiagnosticBag,
) -> Result<(), Error> {
    f.body = rewrite_function_body(arena, f.body, by_site, diagnostics)?;
    Ok(())
}

fn block_site_key(block: &dream_syntax::nodes::SyntaxBlockNode<'_>) -> (String, String) {
    let mut body_text = String::new();
    for part in &block.parts {
        match part {
            SyntaxBlockPart::Text(t) => body_text.push_str(t),
            SyntaxBlockPart::Splice(e) => {
                body_text.push('{');
                body_text.push_str(&super::syntax::expr_source_approx_pub(e));
                body_text.push('}');
            }
        }
    }
    (block.name.text.clone(), body_text)
}

fn rewrite_expr<'a>(
    arena: &'a Bump,
    expr: &ExpressionNode<'a>,
    by_site: &IndexMap<(String, String), String>,
    diagnostics: &mut DiagnosticBag,
    changed: &mut bool,
) -> Result<ExpressionNode<'a>, Error> {
    if let ExpressionNode::SyntaxBlock(block) = expr {
        let key = block_site_key(block);
        if let Some(src) = by_site.get(&key) {
            *changed = true;
            return parse_expression_source(arena, src, diagnostics);
        }
    }
    Ok(match expr {
        ExpressionNode::Binary(l, op, r) => ExpressionNode::Binary(
            arena.alloc(rewrite_expr(arena, l, by_site, diagnostics, changed)?),
            op.clone(),
            arena.alloc(rewrite_expr(arena, r, by_site, diagnostics, changed)?),
        ),
        ExpressionNode::Ternary(c, t, e) => ExpressionNode::Ternary(
            arena.alloc(rewrite_expr(arena, c, by_site, diagnostics, changed)?),
            arena.alloc(rewrite_expr(arena, t, by_site, diagnostics, changed)?),
            arena.alloc(rewrite_expr(arena, e, by_site, diagnostics, changed)?),
        ),
        ExpressionNode::Unary(op, x) => ExpressionNode::Unary(
            op.clone(),
            arena.alloc(rewrite_expr(arena, x, by_site, diagnostics, changed)?),
        ),
        ExpressionNode::Parenthesized(x) => ExpressionNode::Parenthesized(arena.alloc(
            rewrite_expr(arena, x, by_site, diagnostics, changed)?,
        )),
        ExpressionNode::Await(x) => {
            ExpressionNode::Await(arena.alloc(rewrite_expr(arena, x, by_site, diagnostics, changed)?))
        }
        ExpressionNode::Try(x) => {
            ExpressionNode::Try(arena.alloc(rewrite_expr(arena, x, by_site, diagnostics, changed)?))
        }
        ExpressionNode::Cast(ty, x) => ExpressionNode::Cast(
            ty.clone(),
            arena.alloc(rewrite_expr(arena, x, by_site, diagnostics, changed)?),
        ),
        ExpressionNode::IsExpression(x, ty, b) => ExpressionNode::IsExpression(
            arena.alloc(rewrite_expr(arena, x, by_site, diagnostics, changed)?),
            ty.clone(),
            b.clone(),
        ),
        ExpressionNode::IndexAccess(a, i) => ExpressionNode::IndexAccess(
            arena.alloc(rewrite_expr(arena, a, by_site, diagnostics, changed)?),
            arena.alloc(rewrite_expr(arena, i, by_site, diagnostics, changed)?),
        ),
        ExpressionNode::MemberAccess(r, m) => ExpressionNode::MemberAccess(
            arena.alloc(rewrite_expr(arena, r, by_site, diagnostics, changed)?),
            m.clone(),
        ),
        ExpressionNode::RefArgument(x) => ExpressionNode::RefArgument(arena.alloc(
            rewrite_expr(arena, x, by_site, diagnostics, changed)?,
        )),
        ExpressionNode::NamedArg(n, x) => ExpressionNode::NamedArg(
            n.clone(),
            arena.alloc(rewrite_expr(arena, x, by_site, diagnostics, changed)?),
        ),
        ExpressionNode::Call(c, gens, args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            ExpressionNode::Call(
                arena.alloc(rewrite_expr(arena, c, by_site, diagnostics, changed)?),
                gens.clone(),
                nargs,
            )
        }
        ExpressionNode::MethodCall(r, name, gens, args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            ExpressionNode::MethodCall(
                arena.alloc(rewrite_expr(arena, r, by_site, diagnostics, changed)?),
                name.clone(),
                gens.clone(),
                nargs,
            )
        }
        ExpressionNode::FunctionCall(name, gens, args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            ExpressionNode::FunctionCall(name.clone(), gens.clone(), nargs)
        }
        ExpressionNode::ArrayLiteral(args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            ExpressionNode::ArrayLiteral(nargs)
        }
        ExpressionNode::TupleLiteral(args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            ExpressionNode::TupleLiteral(nargs)
        }
        ExpressionNode::SetLiteral(args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            ExpressionNode::SetLiteral(nargs)
        }
        ExpressionNode::MapLiteral(entries) => {
            let mut nentries = Vec::new();
            for (k, v) in entries {
                nentries.push((
                    rewrite_expr(arena, k, by_site, diagnostics, changed)?,
                    rewrite_expr(arena, v, by_site, diagnostics, changed)?,
                ));
            }
            ExpressionNode::MapLiteral(nentries)
        }
        ExpressionNode::Switch(subj, arms) => {
            let nsubj = arena.alloc(rewrite_expr(arena, subj, by_site, diagnostics, changed)?);
            let mut narms = Vec::new();
            for arm in arms {
                let guard = match &arm.guard {
                    Some(g) => Some(rewrite_expr(arena, g, by_site, diagnostics, changed)?),
                    None => None,
                };
                let body = match &arm.body {
                    SwitchArmBody::Expr(e) => {
                        SwitchArmBody::Expr(rewrite_expr(arena, e, by_site, diagnostics, changed)?)
                    }
                    SwitchArmBody::Block(stmts) => {
                        let nb = rewrite_function_body(arena, stmts, by_site, diagnostics)?;
                        SwitchArmBody::Block(nb)
                    }
                };
                narms.push(SwitchArm {
                    pattern: arm.pattern.clone(),
                    guard,
                    body,
                });
            }
            ExpressionNode::Switch(nsubj, narms)
        }
        ExpressionNode::Lambda(l) => {
            let body = match &l.body {
                LambdaBody::Expr(e) => {
                    LambdaBody::Expr(arena.alloc(rewrite_expr(arena, e, by_site, diagnostics, changed)?))
                }
                LambdaBody::Block(stmts) => {
                    LambdaBody::Block(rewrite_function_body(arena, stmts, by_site, diagnostics)?)
                }
            };
            let mut nl = (*l).clone();
            nl.body = body;
            ExpressionNode::Lambda(arena.alloc(nl))
        }
        ExpressionNode::Literal(_)
        | ExpressionNode::Identifier(_)
        | ExpressionNode::SyntaxBlock(_) => expr.clone(),
    })
}

fn rewrite_stmt<'a>(
    arena: &'a Bump,
    stmt: &StatementNode<'a>,
    by_site: &IndexMap<(String, String), String>,
    diagnostics: &mut DiagnosticBag,
    changed: &mut bool,
) -> Result<StatementNode<'a>, Error> {
    Ok(match stmt {
        StatementNode::ExpressionStatement(e) => StatementNode::ExpressionStatement(rewrite_expr(
            arena, e, by_site, diagnostics, changed,
        )?),
        StatementNode::AwaitStmt(e) => {
            StatementNode::AwaitStmt(rewrite_expr(arena, e, by_site, diagnostics, changed)?)
        }
        StatementNode::Return(Some(e)) => {
            StatementNode::Return(Some(rewrite_expr(arena, e, by_site, diagnostics, changed)?))
        }
        StatementNode::Return(None) => StatementNode::Return(None),
        StatementNode::Assignment(n, e) => {
            StatementNode::Assignment(n.clone(), rewrite_expr(arena, e, by_site, diagnostics, changed)?)
        }
        StatementNode::Declaration(n, ty, e, c) => StatementNode::Declaration(
            n.clone(),
            ty.clone(),
            rewrite_expr(arena, e, by_site, diagnostics, changed)?,
            *c,
        ),
        StatementNode::TupleDeclaration {
            names,
            ty,
            init,
            is_const,
        } => StatementNode::TupleDeclaration {
            names: names.clone(),
            ty: ty.clone(),
            init: rewrite_expr(arena, init, by_site, diagnostics, changed)?,
            is_const: *is_const,
        },
        StatementNode::IndexAssignment(a, i, v) => StatementNode::IndexAssignment(
            arena.alloc(rewrite_expr(arena, a, by_site, diagnostics, changed)?),
            arena.alloc(rewrite_expr(arena, i, by_site, diagnostics, changed)?),
            rewrite_expr(arena, v, by_site, diagnostics, changed)?,
        ),
        StatementNode::MemberAssignment(r, m, v) => StatementNode::MemberAssignment(
            arena.alloc(rewrite_expr(arena, r, by_site, diagnostics, changed)?),
            m.clone(),
            rewrite_expr(arena, v, by_site, diagnostics, changed)?,
        ),
        StatementNode::FunctionInvocation(n, g, args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            StatementNode::FunctionInvocation(n.clone(), g.clone(), nargs)
        }
        StatementNode::MethodInvocation(r, n, g, args) => {
            let mut nargs = Vec::new();
            for a in args {
                nargs.push(rewrite_expr(arena, a, by_site, diagnostics, changed)?);
            }
            StatementNode::MethodInvocation(
                arena.alloc(rewrite_expr(arena, r, by_site, diagnostics, changed)?),
                n.clone(),
                g.clone(),
                nargs,
            )
        }
        StatementNode::IfElse(cond, then_b, elifs, else_b) => {
            let ncond = rewrite_expr(arena, cond, by_site, diagnostics, changed)?;
            let nthen = rewrite_function_body(arena, then_b, by_site, diagnostics)?;
            let mut nelifs = Vec::new();
            for (c, b) in elifs {
                nelifs.push((
                    rewrite_expr(arena, c, by_site, diagnostics, changed)?,
                    rewrite_function_body(arena, b, by_site, diagnostics)?,
                ));
            }
            let nelse = match else_b {
                Some(b) => Some(rewrite_function_body(arena, b, by_site, diagnostics)?),
                None => None,
            };
            StatementNode::IfElse(ncond, nthen, nelifs, nelse)
        }
        StatementNode::While(cond, body) => StatementNode::While(
            rewrite_expr(arena, cond, by_site, diagnostics, changed)?,
            rewrite_function_body(arena, body, by_site, diagnostics)?,
        ),
        StatementNode::Lock(cond, body) => StatementNode::Lock(
            rewrite_expr(arena, cond, by_site, diagnostics, changed)?,
            rewrite_function_body(arena, body, by_site, diagnostics)?,
        ),
        StatementNode::DoWhile(body, cond) => StatementNode::DoWhile(
            rewrite_function_body(arena, body, by_site, diagnostics)?,
            rewrite_expr(arena, cond, by_site, diagnostics, changed)?,
        ),
        StatementNode::For(init, cond, inc, body) => {
            let ninit = match init {
                Some(s) => Some(
                    &*arena.alloc(rewrite_stmt(arena, s, by_site, diagnostics, changed)?),
                ),
                None => None,
            };
            let ncond = match cond {
                Some(e) => Some(rewrite_expr(arena, e, by_site, diagnostics, changed)?),
                None => None,
            };
            let ninc = match inc {
                Some(s) => Some(
                    &*arena.alloc(rewrite_stmt(arena, s, by_site, diagnostics, changed)?),
                ),
                None => None,
            };
            StatementNode::For(
                ninit,
                ncond,
                ninc,
                rewrite_function_body(arena, body, by_site, diagnostics)?,
            )
        }
        StatementNode::ForEach(n, iter, a, b, body) => StatementNode::ForEach(
            n.clone(),
            rewrite_expr(arena, iter, by_site, diagnostics, changed)?,
            a.clone(),
            b.clone(),
            rewrite_function_body(arena, body, by_site, diagnostics)?,
        ),
        StatementNode::Switch(subj, cases, default) => {
            let nsubj = rewrite_expr(arena, subj, by_site, diagnostics, changed)?;
            let mut ncases = Vec::new();
            for (labels, body) in cases {
                let mut nlabels = Vec::new();
                for l in labels {
                    nlabels.push(rewrite_expr(arena, l, by_site, diagnostics, changed)?);
                }
                ncases.push((
                    nlabels,
                    rewrite_function_body(arena, body, by_site, diagnostics)?,
                ));
            }
            let ndef = match default {
                Some(b) => Some(rewrite_function_body(arena, b, by_site, diagnostics)?),
                None => None,
            };
            StatementNode::Switch(nsubj, ncases, ndef)
        }
        StatementNode::Labeled(l, inner) => StatementNode::Labeled(
            l.clone(),
            arena.alloc(rewrite_stmt(arena, inner, by_site, diagnostics, changed)?),
        ),
        StatementNode::Break(x) => StatementNode::Break(x.clone()),
        StatementNode::Continue(x) => StatementNode::Continue(x.clone()),
        StatementNode::WorkgroupDecl(n, ty, size) => StatementNode::WorkgroupDecl(n.clone(), ty.clone(), *size),
    })
}
