use super::super::Parser;
use crate::nodes::Visibility;
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {

    /// Parses a top-level variable declaration: an optional `public`/`static` modifier pair,
    /// then `let`/`const`, a name, an optional `: type` annotation, a required initializer, and a
    /// terminating `;`. Returns the assembled [`GlobalVariableNode`].
    pub(crate) fn parse_global_variable(
        &mut self,
    ) -> Result<crate::nodes::GlobalVariableNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();

        // `public`/`internal` and `static` may appear in either order before `let`/`const`.
        let mut visibility = Visibility::Private;
        let mut is_static = false;
        loop {
            if self.try_consume_visibility(&mut visibility) {
                continue;
            }
            match self.current_token().kind {
                TokenKind::StaticToken => {
                    self.match_token(TokenKind::StaticToken);
                    is_static = true;
                }
                _ => break,
            }
        }

        let is_const = self.current_token().kind == TokenKind::ConstToken;
        if is_const {
            self.match_token(TokenKind::ConstToken);
        } else {
            self.match_token(TokenKind::LetToken);
        }

        let mut name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut name, first_trivia);

        let declared_type = if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            Some(self.parse_type()?)
        } else {
            None
        };

        self.match_token(TokenKind::EqualToken);
        let initializer = self.parse_expression(0)?;
        self.match_token(TokenKind::SemicolonToken);

        Ok(crate::nodes::GlobalVariableNode {
            name,
            declared_type,
            initializer,
            is_const,
            visibility,
            is_static,
            file_path: None,
        })
    }
}
