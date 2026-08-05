use super::super::Parser;
use crate::nodes::{
    AttributeNode,
    Visibility,
};
use crate::token::syntax_token::SyntaxToken;
use crate::token::syntax_trivia::SyntaxTrivia;
use crate::token::token_kind::TokenKind;

impl<'a, 'b> Parser<'a, 'b> {
    /// Recovers a doc comment that was attached to a leading attribute. When `first_trivia` (the
    /// trivia captured before attribute parsing) is empty but the first attribute carries leading
    /// trivia (the doc comment was consumed together with `@attr`), returns the attribute's trivia
    /// so it can still be threaded onto the declaration name for hover/LSP.
    pub(crate) fn recover_doc_trivia(
        first_trivia: Vec<SyntaxTrivia>,
        attributes: &[AttributeNode],
    ) -> Vec<SyntaxTrivia> {
        if first_trivia.is_empty() {
            if let Some(first_attr) = attributes.first() {
                if !first_attr.name.leading_trivia.is_empty() {
                    return first_attr.name.leading_trivia.clone();
                }
            }
        }
        first_trivia
    }

    /// Splices recovered doc-comment trivia onto the front of a declaration's name token so tooling
    /// sees the comment on the name even though it lexically preceded attributes/modifiers.
    pub(crate) fn splice_leading_trivia(name: &mut SyntaxToken, trivia: Vec<SyntaxTrivia>) {
        if !trivia.is_empty() {
            name.leading_trivia.splice(0..0, trivia);
        }
    }

    /// Consumes a leading `public`/`internal` modifier token if present, folding it into
    /// `visibility` and reporting a diagnostic if the declaration already carries the other one.
    /// Returns `true` iff a token was consumed, so modifier-parsing loops can detect "no visibility
    /// modifier here" and fall through to their other modifiers/terminator.
    pub(crate) fn try_consume_visibility(&mut self, visibility: &mut Visibility) -> bool {
        let new_vis = match self.current_token().kind {
            TokenKind::PublicToken => Visibility::Public,
            TokenKind::InternalToken => Visibility::Internal,
            _ => return false,
        };
        let position = self.current_token().position;
        self.next_token();
        if *visibility != Visibility::Private && *visibility != new_vis {
            self.diagnostics.report_error(
                "a declaration cannot be both 'public' and 'internal'".to_string(),
                Some(position),
            );
        }
        *visibility = new_vis;
        true
    }
}
