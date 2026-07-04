use super::types::Type;
use crate::token::syntax_token::SyntaxToken;
use dream_text::text_span::TextSpan;

/// A pattern matched by a `match` arm. Patterns may nest (a variant's sub-patterns are themselves
/// patterns), enabling forms like `Pair(Some(x), None)`.
#[derive(Debug, Clone)]
pub enum PatternNode {
    /// `_` - matches anything and binds nothing.
    Wildcard(SyntaxToken),
    /// A bare identifier - matches anything and binds the matched value to the name.
    Binding(SyntaxToken),
    /// A constant literal (`0`, `"s"`, `true`) - matches when the subject equals the literal.
    Literal(Type),
    /// A (discriminated-union) variant pattern: an optional `EnumName.` qualifier, the variant
    /// name, and the sub-patterns for its payload fields (positional, in declaration order).
    /// A unit variant has no sub-patterns.
    Variant(Option<SyntaxToken>, SyntaxToken, Vec<PatternNode>),
}

impl PatternNode {
    /// A representative source span for diagnostics.
    pub fn position(&self) -> Option<TextSpan> {
        match self {
            PatternNode::Wildcard(t) | PatternNode::Binding(t) => Some(t.position),
            PatternNode::Literal(ty) => ty.get_span(),
            PatternNode::Variant(_, name, _) => Some(name.position),
        }
    }
}
