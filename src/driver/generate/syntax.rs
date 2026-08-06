//! Walkable syntax facade for generators (NodeId-based; no arena pointers to Dream).

use dream_syntax::nodes::{ExpressionNode, SyntaxBlockPart};
use dream_text::text_span::TextSpan;
use indexmap::IndexMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyntaxNodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxKind {
    CompilationUnit,
    ClassDecl,
    StructDecl,
    EnumDecl,
    InterfaceDecl,
    FieldDecl,
    MethodDecl,
    ConstructorDecl,
    Parameter,
    Attribute,
    SyntaxBlock,
    Splice,
    Text,
    Other,
}

#[derive(Debug, Clone)]
pub struct SyntaxNodeData {
    pub kind: SyntaxKind,
    pub parent: Option<SyntaxNodeId>,
    pub children: Vec<SyntaxNodeId>,
    pub span: Option<TextSpan>,
    pub text: String,
    pub syntax_block_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct SyntaxTreeView {
    nodes: IndexMap<SyntaxNodeId, SyntaxNodeData>,
    next_id: u32,
    /// Maps syntax-block node id → index into the program's expression sites (for replace).
    pub block_keys: IndexMap<SyntaxNodeId, BlockSite>,
}

#[derive(Debug, Clone)]
pub struct BlockSite {
    /// Introducer name (`html`, …).
    pub name: String,
    /// Raw reconstructed source of the block body (text + splice placeholders).
    pub body_text: String,
    /// Dream source of each splice expression, in order.
    pub splice_sources: Vec<String>,
}

impl SyntaxTreeView {
    pub fn root(&self) -> Option<SyntaxNodeId> {
        self.nodes.keys().next().copied()
    }

    pub fn get(&self, id: SyntaxNodeId) -> Option<&SyntaxNodeData> {
        self.nodes.get(&id)
    }

    pub fn nodes_of_kind(&self, kind: SyntaxKind) -> Vec<SyntaxNodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.kind == kind)
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn syntax_blocks(&self, name: &str) -> Vec<SyntaxNodeId> {
        self.block_keys
            .iter()
            .filter(|(_, site)| site.name == name)
            .map(|(id, _)| *id)
            .collect()
    }

    fn alloc(&mut self, data: SyntaxNodeData) -> SyntaxNodeId {
        let id = SyntaxNodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(id, data);
        id
    }

    /// Indexes every `SyntaxBlock` expression in the merged program for generator replace.
    pub fn index_syntax_blocks_from_exprs<'a>(
        &mut self,
        exprs: impl Iterator<Item = &'a ExpressionNode<'a>>,
    ) {
        for expr in exprs {
            self.walk_expr(expr, None);
        }
    }

    pub fn walk_stmt_public(&mut self, stmt: &dream_syntax::nodes::StatementNode<'_>) {
        self.walk_stmt(stmt, None);
    }

    pub fn walk_expr_public(&mut self, expr: &ExpressionNode<'_>) {
        self.walk_expr(expr, None);
    }

    fn walk_expr(&mut self, expr: &ExpressionNode<'_>, parent: Option<SyntaxNodeId>) {
        match expr {
            ExpressionNode::SyntaxBlock(block) => {
                let mut body_text = String::new();
                let mut splice_sources = Vec::new();
                let mut children = Vec::new();
                for part in &block.parts {
                    match part {
                        SyntaxBlockPart::Text(t) => {
                            body_text.push_str(t);
                            let cid = self.alloc(SyntaxNodeData {
                                kind: SyntaxKind::Text,
                                parent: None,
                                children: Vec::new(),
                                span: None,
                                text: t.clone(),
                                syntax_block_name: String::new(),
                            });
                            children.push(cid);
                        }
                        SyntaxBlockPart::Splice(e) => {
                            let src = expr_source_approx(e);
                            body_text.push('{');
                            body_text.push_str(&src);
                            body_text.push('}');
                            splice_sources.push(src);
                            let cid = self.alloc(SyntaxNodeData {
                                kind: SyntaxKind::Splice,
                                parent: None,
                                children: Vec::new(),
                                span: e.position(),
                                text: expr_source_approx(e),
                                syntax_block_name: String::new(),
                            });
                            children.push(cid);
                            self.walk_expr(e, Some(cid));
                        }
                    }
                }
                let id = self.alloc(SyntaxNodeData {
                    kind: SyntaxKind::SyntaxBlock,
                    parent,
                    children: children.clone(),
                    span: Some(block.name.position),
                    text: body_text.clone(),
                    syntax_block_name: block.name.text.clone(),
                });
                for c in &children {
                    if let Some(n) = self.nodes.get_mut(c) {
                        n.parent = Some(id);
                    }
                }
                self.block_keys.insert(
                    id,
                    BlockSite {
                        name: block.name.text.clone(),
                        body_text,
                        splice_sources,
                    },
                );
            }
            ExpressionNode::Binary(l, _, r) | ExpressionNode::Ternary(l, r, _) => {
                self.walk_expr(l, parent);
                self.walk_expr(r, parent);
            }
            ExpressionNode::IndexAccess(a, i) => {
                self.walk_expr(a, parent);
                self.walk_expr(i, parent);
            }
            ExpressionNode::Unary(_, x)
            | ExpressionNode::Parenthesized(x)
            | ExpressionNode::Await(x)
            | ExpressionNode::Try(x)
            | ExpressionNode::Cast(_, x)
            | ExpressionNode::IsExpression(x, _, _)
            | ExpressionNode::MemberAccess(x, _)
            | ExpressionNode::RefArgument(x)
            | ExpressionNode::NamedArg(_, x) => self.walk_expr(x, parent),
            ExpressionNode::Call(c, _, args) | ExpressionNode::MethodCall(c, _, _, args) => {
                self.walk_expr(c, parent);
                for a in args {
                    self.walk_expr(a, parent);
                }
            }
            ExpressionNode::FunctionCall(_, _, args)
            | ExpressionNode::ArrayLiteral(args)
            | ExpressionNode::TupleLiteral(args)
            | ExpressionNode::SetLiteral(args) => {
                for a in args {
                    self.walk_expr(a, parent);
                }
            }
            ExpressionNode::MapLiteral(entries) => {
                for (k, v) in entries {
                    self.walk_expr(k, parent);
                    self.walk_expr(v, parent);
                }
            }
            ExpressionNode::Switch(subj, arms) => {
                self.walk_expr(subj, parent);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.walk_expr(g, parent);
                    }
                    match &arm.body {
                        dream_syntax::nodes::SwitchArmBody::Expr(e) => self.walk_expr(e, parent),
                        dream_syntax::nodes::SwitchArmBody::Block(stmts) => {
                            for s in *stmts {
                                self.walk_stmt(s, parent);
                            }
                        }
                    }
                }
            }
            ExpressionNode::Lambda(l) => match &l.body {
                dream_syntax::nodes::LambdaBody::Expr(e) => self.walk_expr(e, parent),
                dream_syntax::nodes::LambdaBody::Block(stmts) => {
                    for s in *stmts {
                        self.walk_stmt(s, parent);
                    }
                }
            },
            ExpressionNode::Literal(_) | ExpressionNode::Identifier(_) => {}
        }
    }

    fn walk_stmt(&mut self, stmt: &dream_syntax::nodes::StatementNode<'_>, parent: Option<SyntaxNodeId>) {
        use dream_syntax::nodes::StatementNode;
        match stmt {
            StatementNode::ExpressionStatement(e)
            | StatementNode::AwaitStmt(e)
            | StatementNode::Return(Some(e))
            | StatementNode::Assignment(_, e)
            | StatementNode::Declaration(_, _, e, _)
            | StatementNode::TupleDeclaration { init: e, .. } => self.walk_expr(e, parent),
            StatementNode::IndexAssignment(a, i, v) => {
                self.walk_expr(a, parent);
                self.walk_expr(i, parent);
                self.walk_expr(v, parent);
            }
            StatementNode::MemberAssignment(r, _, v) => {
                self.walk_expr(r, parent);
                self.walk_expr(v, parent);
            }
            StatementNode::FunctionInvocation(_, _, args) => {
                for a in args {
                    self.walk_expr(a, parent);
                }
            }
            StatementNode::MethodInvocation(r, _, _, args) => {
                self.walk_expr(r, parent);
                for a in args {
                    self.walk_expr(a, parent);
                }
            }
            StatementNode::IfElse(cond, then_b, elifs, else_b) => {
                self.walk_expr(cond, parent);
                for s in *then_b {
                    self.walk_stmt(s, parent);
                }
                for (c, b) in elifs {
                    self.walk_expr(c, parent);
                    for s in *b {
                        self.walk_stmt(s, parent);
                    }
                }
                if let Some(eb) = else_b {
                    for s in *eb {
                        self.walk_stmt(s, parent);
                    }
                }
            }
            StatementNode::While(cond, body) | StatementNode::Lock(cond, body) => {
                self.walk_expr(cond, parent);
                for s in *body {
                    self.walk_stmt(s, parent);
                }
            }
            StatementNode::DoWhile(body, cond) => {
                for s in *body {
                    self.walk_stmt(s, parent);
                }
                self.walk_expr(cond, parent);
            }
            StatementNode::For(init, cond, inc, body) => {
                if let Some(s) = init {
                    self.walk_stmt(s, parent);
                }
                if let Some(e) = cond {
                    self.walk_expr(e, parent);
                }
                if let Some(s) = inc {
                    self.walk_stmt(s, parent);
                }
                for s in *body {
                    self.walk_stmt(s, parent);
                }
            }
            StatementNode::ForEach(_, iter, _, _, body) => {
                self.walk_expr(iter, parent);
                for s in *body {
                    self.walk_stmt(s, parent);
                }
            }
            StatementNode::Switch(subj, cases, default) => {
                self.walk_expr(subj, parent);
                for (labels, body) in cases {
                    for l in labels {
                        self.walk_expr(l, parent);
                    }
                    for s in *body {
                        self.walk_stmt(s, parent);
                    }
                }
                if let Some(d) = default {
                    for s in *d {
                        self.walk_stmt(s, parent);
                    }
                }
            }
            StatementNode::Labeled(_, inner) => self.walk_stmt(inner, parent),
            StatementNode::Return(None)
            | StatementNode::Break(_)
            | StatementNode::Continue(_)
            | StatementNode::WorkgroupDecl(_, _, _) => {}
        }
    }
}

/// Best-effort source reconstruction for splice expressions (identifiers and simple forms).
pub fn expr_source_approx_pub(expr: &ExpressionNode<'_>) -> String {
    expr_source_approx(expr)
}

fn expr_source_approx(expr: &ExpressionNode<'_>) -> String {
    match expr {
        ExpressionNode::Identifier(t) => t.text.clone(),
        ExpressionNode::Literal(t) => match t {
            dream_syntax::nodes::Type::Integer(tok)
            | dream_syntax::nodes::Type::Float(tok)
            | dream_syntax::nodes::Type::Double(tok)
            | dream_syntax::nodes::Type::String(tok)
            | dream_syntax::nodes::Type::Boolean(tok)
            | dream_syntax::nodes::Type::Char(tok)
            | dream_syntax::nodes::Type::Long(tok)
            | dream_syntax::nodes::Type::UInt(tok)
            | dream_syntax::nodes::Type::ULong(tok)
            | dream_syntax::nodes::Type::Byte(tok) => tok.text.clone(),
            _ => "/*lit*/".into(),
        },
        ExpressionNode::MemberAccess(recv, mem) => {
            format!("{}.{}", expr_source_approx(recv), mem.text)
        }
        ExpressionNode::Binary(l, op, r) => {
            format!(
                "{} {} {}",
                expr_source_approx(l),
                op.text,
                expr_source_approx(r)
            )
        }
        ExpressionNode::Parenthesized(inner) => format!("({})", expr_source_approx(inner)),
        _ => "/*expr*/".into(),
    }
}
