use super::super::Parser;
use crate::token::token_kind::TokenKind;

impl<'a, 'b> Parser<'a, 'b> {

    pub(crate) fn parse_attributes(&mut self) -> Vec<crate::nodes::AttributeNode> {
        let mut attributes = Vec::new();
        while self.current_token().kind == TokenKind::AtToken {
            let at = self.match_token(TokenKind::AtToken);
            let mut name = self.match_token(TokenKind::IdentifierToken);
            // A doc comment preceding the declaration attaches to the `@` token (the first token of
            // the declaration). Thread it onto the attribute name so tooling can recover it even
            // when the attribute is parsed before the `fun`/`class` keyword.
            if !at.leading_trivia.is_empty() {
                name.leading_trivia.splice(0..0, at.leading_trivia);
            }
            let mut args = Vec::new();
            if self.current_token().kind == TokenKind::OpenParenthesisToken {
                self.match_token(TokenKind::OpenParenthesisToken);
                while self.current_token().kind != TokenKind::CloseParenthesisToken
                    && self.current_token().kind != TokenKind::EndOfFileToken
                {
                    let iter = self.current_token_index;
                    args.push(self.current_token().clone());
                    self.next_token();
                    if self.current_token().kind == TokenKind::CommaToken {
                        self.match_token(TokenKind::CommaToken);
                    }
                    self.ensure_progress(iter);
                }
                self.match_token(TokenKind::CloseParenthesisToken);
            }
            attributes.push(crate::nodes::AttributeNode { name, args });
        }
        attributes
    }
}
