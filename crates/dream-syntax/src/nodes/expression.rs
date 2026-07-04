use super::pattern::PatternNode;
use super::statement::StatementNode;
use super::types::Type;
use crate::token::syntax_token::SyntaxToken;
use dream_text::text_span::TextSpan;

/// Represents an expression node in the AST
#[derive(Debug, Clone)]
pub enum ExpressionNode<'a> {
    Literal(Type),
    ArrayLiteral(Vec<ExpressionNode<'a>>),
    Binary(&'a ExpressionNode<'a>, SyntaxToken, &'a ExpressionNode<'a>),
    Unary(SyntaxToken, &'a ExpressionNode<'a>),
    Identifier(SyntaxToken),
    Parenthesized(&'a ExpressionNode<'a>),
    FunctionCall(SyntaxToken, Option<Vec<Type>>, Vec<ExpressionNode<'a>>),
    IndexAccess(&'a ExpressionNode<'a>, &'a ExpressionNode<'a>),
    Cast(Type, &'a ExpressionNode<'a>),
    MemberAccess(&'a ExpressionNode<'a>, SyntaxToken),
    /// `expr is Type` — a runtime type check. The optional trailing `SyntaxToken` is an
    /// `is`-with-binding name (`expr is Type name`): when present, the analyzer introduces a new
    /// local `name: Type` (narrowed from `expr`) scoped to the branch guarded by the check.
    IsExpression(&'a ExpressionNode<'a>, Type, Option<SyntaxToken>),
    MethodCall(
        &'a ExpressionNode<'a>,
        SyntaxToken,
        Option<Vec<Type>>,
        Vec<ExpressionNode<'a>>,
    ),
    /// `condition ? then_value : else_value`
    Ternary(
        &'a ExpressionNode<'a>,
        &'a ExpressionNode<'a>,
        &'a ExpressionNode<'a>,
    ),
    /// `await <future-expr>`: suspends the enclosing `async` function until the awaited
    /// `Future<T>` resolves, then yields its `T`. The inner expression produces the future.
    Await(&'a ExpressionNode<'a>),
    /// `switch (subject) { pattern [if guard] => body, ... }` in its pattern-matching form. Used
    /// both as an expression (every arm yields a value of a common type) and, when wrapped in an
    /// `ExpressionStatement`, as a statement (arms may be blocks yielding `void`). The first field
    /// is the subject. (The C-style `switch` with `case`/`default` is `StatementNode::Switch`.)
    Switch(&'a ExpressionNode<'a>, Vec<SwitchArm<'a>>),
}

/// One arm of a pattern-matching `switch`: a pattern, an optional `if` guard, and a body.
#[derive(Debug, Clone)]
pub struct SwitchArm<'a> {
    pub pattern: PatternNode,
    /// An optional `if <bool-expr>` guard; the arm only matches when the guard is also true.
    pub guard: Option<ExpressionNode<'a>>,
    pub body: SwitchArmBody<'a>,
}

/// The body of a pattern-matching `switch` arm.
// The `Expr` variant embeds a full `ExpressionNode`, which is larger than the slice-backed `Block`
// variant; boxing it would penalize the common expression form for no real memory win in the AST.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SwitchArmBody<'a> {
    /// `=> expr` - yields the expression's value (the only form allowed in expression position).
    Expr(ExpressionNode<'a>),
    /// `=> { stmts }` - a statement block yielding `void` (only allowed in statement position).
    Block(&'a [StatementNode<'a>]),
}

impl<'a> ExpressionNode<'a> {
    /// Returns a representative source span for this expression, derived from an existing
    /// token in the node (no positions are stored separately). Used to attach line/column
    /// information to semantic diagnostics. Returns `None` only when nothing positional is
    /// available (e.g. an empty array literal or the `null` literal).
    pub fn position(&self) -> Option<TextSpan> {
        match self {
            ExpressionNode::Literal(t) => t.get_span(),
            ExpressionNode::Identifier(token)
            | ExpressionNode::FunctionCall(token, _, _)
            | ExpressionNode::MemberAccess(_, token)
            | ExpressionNode::MethodCall(_, token, _, _)
            | ExpressionNode::Binary(_, token, _)
            | ExpressionNode::Unary(token, _) => Some(token.position),
            ExpressionNode::Parenthesized(inner)
            | ExpressionNode::Await(inner)
            | ExpressionNode::IsExpression(inner, _, _) => inner.position(),
            ExpressionNode::Switch(subject, _) => subject.position(),
            ExpressionNode::Ternary(cond, _, _) => cond.position(),
            ExpressionNode::IndexAccess(array_expr, _) => array_expr.position(),
            ExpressionNode::Cast(target_type, expr) => {
                target_type.get_span().or_else(|| expr.position())
            }
            ExpressionNode::ArrayLiteral(elements) => elements.first().and_then(|e| e.position()),
        }
    }

    /// Returns the span of the *leftmost* token of this expression (its true start), as opposed to
    /// [`position`](Self::position), which returns a representative interior token. For `a.b` this is
    /// `a` (not the `.b` member), for `a * n` it is `a` (not the operator), for `f(x).g()` it is `f`.
    /// Used where the start offset matters — e.g. placing a parameter-name inlay hint *before* a call
    /// argument rather than in the middle of it.
    pub fn start_position(&self) -> Option<TextSpan> {
        match self {
            ExpressionNode::MemberAccess(receiver, _) => {
                receiver.start_position().or_else(|| self.position())
            }
            ExpressionNode::MethodCall(receiver, _, _, _) => receiver.start_position(),
            ExpressionNode::Binary(left, _, _) => left.start_position(),
            ExpressionNode::IndexAccess(array_expr, _) => array_expr.start_position(),
            ExpressionNode::Parenthesized(inner)
            | ExpressionNode::Await(inner)
            | ExpressionNode::IsExpression(inner, _, _) => inner.start_position(),
            ExpressionNode::Switch(subject, _) => subject.start_position(),
            ExpressionNode::Ternary(cond, _, _) => cond.start_position(),
            ExpressionNode::ArrayLiteral(elements) => {
                elements.first().and_then(|e| e.start_position())
            }
            // Token-led forms (identifier, call name, unary operator, cast type, literal) already
            // start at the token `position` returns.
            _ => self.position(),
        }
    }
}
