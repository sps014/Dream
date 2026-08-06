use super::expression::ExpressionNode;
use super::types::Type;
use crate::token::syntax_token::SyntaxToken;

/// Represents a statement node in the AST
#[derive(Debug, Clone)]
pub enum StatementNode<'a> {
    Assignment(SyntaxToken, ExpressionNode<'a>),
    IndexAssignment(
        &'a ExpressionNode<'a>,
        &'a ExpressionNode<'a>,
        ExpressionNode<'a>,
    ),
    MemberAssignment(&'a ExpressionNode<'a>, SyntaxToken, ExpressionNode<'a>),
    /// `let`/`const` declaration. The final `bool` marks `const` (immutable) bindings.
    Declaration(SyntaxToken, Option<Type>, ExpressionNode<'a>, bool),
    /// `let (a, b) = expr;` / `const (a, b): (T, U) = expr;` — positional tuple destructure.
    /// Names are identifiers only (no nested patterns). Arity ≥ 2.
    TupleDeclaration {
        names: Vec<SyntaxToken>,
        ty: Option<Type>,
        init: ExpressionNode<'a>,
        is_const: bool,
    },
    FunctionInvocation(SyntaxToken, Option<Vec<Type>>, Vec<ExpressionNode<'a>>),
    MethodInvocation(
        &'a ExpressionNode<'a>,
        SyntaxToken,
        Option<Vec<Type>>,
        Vec<ExpressionNode<'a>>,
    ),
    Return(Option<ExpressionNode<'a>>),
    /// If condition, then body, else if pairs, else body
    IfElse(
        ExpressionNode<'a>,
        &'a [StatementNode<'a>],
        Vec<(ExpressionNode<'a>, &'a [StatementNode<'a>])>,
        Option<&'a [StatementNode<'a>]>,
    ),
    While(ExpressionNode<'a>, &'a [StatementNode<'a>]),
    /// `do { body } while (condition);` - the body always runs at least once.
    DoWhile(&'a [StatementNode<'a>], ExpressionNode<'a>),
    For(
        Option<&'a StatementNode<'a>>,
        Option<ExpressionNode<'a>>,
        Option<&'a StatementNode<'a>>,
        &'a [StatementNode<'a>],
    ),
    /// A labeled loop: `label: while (...) { ... }`. Wraps a single loop statement so that
    /// `break label;` / `continue label;` can target it.
    Labeled(String, &'a StatementNode<'a>),
    /// `break` / `continue`, optionally targeting an enclosing labeled loop.
    Break(Option<String>),
    Continue(Option<String>),
    /// An expression used as a statement, typically missing an assignment or call.
    ExpressionStatement(ExpressionNode<'a>),
    /// `await <future-expr>;` used as a statement, discarding the resolved value. The inner
    /// expression produces the `Future` to await (it is NOT wrapped in `ExpressionNode::Await`).
    AwaitStmt(ExpressionNode<'a>),
    /// `for (let element in iterable) { body }`. Iterates the elements of an array. The two
    /// `String` fields are unique synthetic local names (index counter and array temp) generated
    /// by the parser so codegen can lower this to an index loop without re-evaluating `iterable`.
    ForEach(
        SyntaxToken,
        ExpressionNode<'a>,
        String,
        String,
        &'a [StatementNode<'a>],
    ),
    /// `switch (subject) { case labels: body ... default: body }`. Each case carries one or more
    /// constant label expressions and its own body; there is no implicit fallthrough. The final
    /// element is the optional `default` body.
    Switch(
        ExpressionNode<'a>,
        Vec<(Vec<ExpressionNode<'a>>, &'a [StatementNode<'a>])>,
        Option<&'a [StatementNode<'a>]>,
    ),
    /// `lock (target) { body }` — mutual exclusion on `target` (must be `@shared class`-typed, or
    /// `Lock`), reentrant per-thread. See `docs/language` webworkers/concurrency notes.
    Lock(ExpressionNode<'a>, &'a [StatementNode<'a>]),
}
