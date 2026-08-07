//! Compile-time source generators: declaration SemanticModel, walkable SyntaxTree facade,
//! GeneratorContext, registration, and the generate/merge pipeline.

mod context;
mod json_gen;
mod manifest;
mod registration;
mod rewrite;
mod semantic;
mod syntax;
mod syntax_gen;

pub use context::GeneratorContext;
pub use registration::{discover_generators, RegisteredGenerator};
pub use semantic::{
    SemanticModel, Symbol, SymbolKind, TypeKind, TypeSymbol, Visibility as SymVisibility,
};
pub use syntax::{SyntaxKind, SyntaxNodeId, SyntaxTreeView};

use bumpalo::Bump;
use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::{ExpressionNode, StatementNode};
use std::collections::HashMap;
use std::io::Error;

use crate::driver::source_loader::ProgramAccumulator;

/// Runs the declaration + syntax generate passes.
pub fn run_generators<'a>(
    arena: &'a Bump,
    acc: &mut ProgramAccumulator<'a>,
    entry_file: &str,
    diagnostics: &mut DiagnosticBag,
) -> Result<(), Error> {
    acc.manifest_generator_paths = manifest::load_manifest_generators(entry_file);

    let registered = discover_generators(acc, diagnostics);
    let mut ctx = GeneratorContext::build(acc, registered);

    json_gen::expand_from_acc(&mut ctx, &acc.all_structs, &acc.all_enums, diagnostics);
    ctx.apply_emits(arena, acc, diagnostics)?;

    syntax_gen::expand_syntax_blocks(&mut ctx, diagnostics);

    ctx.apply_replacements(arena, acc, diagnostics)?;
    // Flush any `ctx.error` reported during replacements (apply_emits already flushed once).
    ctx.flush_errors(diagnostics);

    report_unexpanded_syntax_blocks(acc, diagnostics);
    Ok(())
}

fn report_unexpanded_syntax_blocks(acc: &ProgramAccumulator<'_>, diagnostics: &mut DiagnosticBag) {
    fn walk_expr(expr: &ExpressionNode<'_>, diagnostics: &mut DiagnosticBag) {
        match expr {
            ExpressionNode::SyntaxBlock(block) => {
                diagnostics.report_error(
                    format!(
                        "unexpanded syntax block '{}'; no generator registered for this introducer",
                        block.name.text
                    ),
                    Some(block.name.position),
                );
            }
            ExpressionNode::Binary(l, _, r) => {
                walk_expr(l, diagnostics);
                walk_expr(r, diagnostics);
            }
            ExpressionNode::Ternary(c, t, e) => {
                walk_expr(c, diagnostics);
                walk_expr(t, diagnostics);
                walk_expr(e, diagnostics);
            }
            ExpressionNode::IndexAccess(a, i) => {
                walk_expr(a, diagnostics);
                walk_expr(i, diagnostics);
            }
            ExpressionNode::Unary(_, x)
            | ExpressionNode::IncDec { target: x, .. }
            | ExpressionNode::Parenthesized(x)
            | ExpressionNode::Await(x)
            | ExpressionNode::Try(x)
            | ExpressionNode::Cast(_, x)
            | ExpressionNode::IsExpression(x, _, _)
            | ExpressionNode::MemberAccess(x, _)
            | ExpressionNode::RefArgument(x)
            | ExpressionNode::NamedArg(_, x) => walk_expr(x, diagnostics),
            ExpressionNode::Call(c, _, args) | ExpressionNode::MethodCall(c, _, _, args) => {
                walk_expr(c, diagnostics);
                for a in args {
                    walk_expr(a, diagnostics);
                }
            }
            ExpressionNode::FunctionCall(_, _, args)
            | ExpressionNode::ArrayLiteral(args)
            | ExpressionNode::TupleLiteral(args)
            | ExpressionNode::SetLiteral(args) => {
                for a in args {
                    walk_expr(a, diagnostics);
                }
            }
            ExpressionNode::MapLiteral(entries) => {
                for (k, v) in entries {
                    walk_expr(k, diagnostics);
                    walk_expr(v, diagnostics);
                }
            }
            ExpressionNode::Switch(subj, arms) => {
                walk_expr(subj, diagnostics);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        walk_expr(g, diagnostics);
                    }
                    match &arm.body {
                        dream_syntax::nodes::SwitchArmBody::Expr(e) => walk_expr(e, diagnostics),
                        dream_syntax::nodes::SwitchArmBody::Block(stmts) => {
                            for s in *stmts {
                                walk_stmt(s, diagnostics);
                            }
                        }
                    }
                }
            }
            ExpressionNode::Lambda(l) => match &l.body {
                dream_syntax::nodes::LambdaBody::Expr(e) => walk_expr(e, diagnostics),
                dream_syntax::nodes::LambdaBody::Block(stmts) => {
                    for s in *stmts {
                        walk_stmt(s, diagnostics);
                    }
                }
            },
            ExpressionNode::Literal(_) | ExpressionNode::Identifier(_) => {}
        }
    }
    fn walk_stmt(stmt: &StatementNode<'_>, diagnostics: &mut DiagnosticBag) {
        match stmt {
            StatementNode::ExpressionStatement(e)
            | StatementNode::AwaitStmt(e)
            | StatementNode::Return(Some(e))
            | StatementNode::Assignment(_, e)
            | StatementNode::Declaration(_, _, e, _)
            | StatementNode::TupleDeclaration { init: e, .. } => walk_expr(e, diagnostics),
            StatementNode::IndexAssignment(a, i, v) => {
                walk_expr(a, diagnostics);
                walk_expr(i, diagnostics);
                walk_expr(v, diagnostics);
            }
            StatementNode::MemberAssignment(r, _, v) => {
                walk_expr(r, diagnostics);
                walk_expr(v, diagnostics);
            }
            StatementNode::FunctionInvocation(_, _, args) => {
                for a in args {
                    walk_expr(a, diagnostics);
                }
            }
            StatementNode::MethodInvocation(r, _, _, args) => {
                walk_expr(r, diagnostics);
                for a in args {
                    walk_expr(a, diagnostics);
                }
            }
            StatementNode::IfElse(cond, then_b, elifs, else_b) => {
                walk_expr(cond, diagnostics);
                for s in *then_b {
                    walk_stmt(s, diagnostics);
                }
                for (c, b) in elifs {
                    walk_expr(c, diagnostics);
                    for s in *b {
                        walk_stmt(s, diagnostics);
                    }
                }
                if let Some(eb) = else_b {
                    for s in *eb {
                        walk_stmt(s, diagnostics);
                    }
                }
            }
            StatementNode::While(cond, body) | StatementNode::Lock(cond, body) => {
                walk_expr(cond, diagnostics);
                for s in *body {
                    walk_stmt(s, diagnostics);
                }
            }
            StatementNode::DoWhile(body, cond) => {
                for s in *body {
                    walk_stmt(s, diagnostics);
                }
                walk_expr(cond, diagnostics);
            }
            StatementNode::For(init, cond, inc, body) => {
                if let Some(s) = init {
                    walk_stmt(s, diagnostics);
                }
                if let Some(e) = cond {
                    walk_expr(e, diagnostics);
                }
                if let Some(s) = inc {
                    walk_stmt(s, diagnostics);
                }
                for s in *body {
                    walk_stmt(s, diagnostics);
                }
            }
            StatementNode::ForEach(_, iter, _, _, body) => {
                walk_expr(iter, diagnostics);
                for s in *body {
                    walk_stmt(s, diagnostics);
                }
            }
            StatementNode::Switch(subj, cases, default) => {
                walk_expr(subj, diagnostics);
                for (labels, body) in cases {
                    for l in labels {
                        walk_expr(l, diagnostics);
                    }
                    for s in *body {
                        walk_stmt(s, diagnostics);
                    }
                }
                if let Some(d) = default {
                    for s in *d {
                        walk_stmt(s, diagnostics);
                    }
                }
            }
            StatementNode::Labeled(_, inner) => walk_stmt(inner, diagnostics),
            StatementNode::Return(None)
            | StatementNode::Break(_)
            | StatementNode::Continue(_)
            | StatementNode::WorkgroupDecl(_, _, _) => {}
        }
    }

    for f in &acc.all_functions {
        for s in f.body {
            walk_stmt(s, diagnostics);
        }
    }
    for s in &acc.all_structs {
        for m in &s.methods {
            for st in m.body {
                walk_stmt(st, diagnostics);
            }
        }
    }
    for e in &acc.all_extends {
        for m in &e.methods {
            for st in m.body {
                walk_stmt(st, diagnostics);
            }
        }
    }
}

/// Parse Dream source containing `extend` blocks into `ExtendNode`s.
pub(crate) fn parse_extends<'a>(
    arena: &'a Bump,
    source: String,
    synthetic_path: &str,
    diagnostics: &mut DiagnosticBag,
    file_contents: &mut HashMap<String, String>,
) -> Result<Vec<dream_syntax::nodes::ExtendNode<'a>>, Error> {
    use dream_syntax::lexer::Lexer;
    use dream_syntax::parser::Parser;

    file_contents.insert(synthetic_path.to_string(), source.clone());
    let mut derive_diagnostics = DiagnosticBag::new(Some(synthetic_path.to_string()));
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer, arena, &mut derive_diagnostics);
    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(e) => {
            diagnostics.extend(&derive_diagnostics);
            return Err(e);
        }
    };
    diagnostics.extend(&derive_diagnostics);
    let program = ast.get_root();
    let file_tag: std::rc::Rc<str> = std::rc::Rc::from(synthetic_path);
    let mut out = Vec::new();
    for extend_decl in program.extends.iter().cloned() {
        let mut extend_decl = extend_decl;
        extend_decl.file_path = Some(file_tag.clone());
        extend_decl.is_synthesized = true;
        for method in extend_decl.methods.iter_mut() {
            method.file_path = None;
        }
        out.push(extend_decl);
    }
    Ok(out)
}
