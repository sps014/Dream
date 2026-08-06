use super::super::Parser;
use crate::nodes::AttributeArg;
use crate::token::syntax_token::SyntaxToken;
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
                    if let Some(arg) = self.parse_attribute_arg() {
                        args.push(arg);
                    }
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

    /// Parse one attribute argument: string / bool / number (typed by suffix) / dotted enum path.
    fn parse_attribute_arg(&mut self) -> Option<AttributeArg> {
        let cur = self.current_token();
        match cur.kind {
            TokenKind::StringToken => {
                let t = self.match_token(TokenKind::StringToken);
                Some(AttributeArg::String(t))
            }
            TokenKind::BooleanToken => {
                let t = self.match_token(TokenKind::BooleanToken);
                Some(AttributeArg::Bool(t))
            }
            TokenKind::NumberToken | TokenKind::MinusToken => {
                let negative = cur.kind == TokenKind::MinusToken;
                if negative {
                    self.match_token(TokenKind::MinusToken);
                }
                let token = self.match_token(TokenKind::NumberToken);
                let ty = Self::classify_number_literal(token);
                let signed = |mut t: SyntaxToken| {
                    if negative {
                        t.text = format!("-{}", t.text);
                    }
                    t
                };
                Some(match ty {
                    crate::nodes::Type::Float(t) => AttributeArg::Float(signed(t)),
                    crate::nodes::Type::Double(t) => AttributeArg::Double(signed(t)),
                    crate::nodes::Type::Integer(t)
                    | crate::nodes::Type::Long(t)
                    | crate::nodes::Type::ULong(t)
                    | crate::nodes::Type::UInt(t)
                    | crate::nodes::Type::Byte(t) => AttributeArg::Int(signed(t)),
                    other => {
                        // Shouldn't happen for number tokens; recover as int "0".
                        let span = match &other {
                            crate::nodes::Type::Integer(t)
                            | crate::nodes::Type::Float(t)
                            | crate::nodes::Type::Double(t)
                            | crate::nodes::Type::Long(t)
                            | crate::nodes::Type::ULong(t)
                            | crate::nodes::Type::UInt(t)
                            | crate::nodes::Type::Byte(t)
                            | crate::nodes::Type::Boolean(t)
                            | crate::nodes::Type::Char(t)
                            | crate::nodes::Type::String(t) => t.position,
                            _ => cur.position,
                        };
                        AttributeArg::Int(SyntaxToken::new(
                            TokenKind::NumberToken,
                            span,
                            "0".into(),
                        ))
                    }
                })
            }
            TokenKind::IdentifierToken => {
                let mut parts = vec![self.match_token(TokenKind::IdentifierToken)];
                while self.current_token().kind == TokenKind::DotToken {
                    self.match_token(TokenKind::DotToken);
                    if self.current_token().kind == TokenKind::IdentifierToken {
                        parts.push(self.match_token(TokenKind::IdentifierToken));
                    } else {
                        break;
                    }
                }
                Some(AttributeArg::Enum(parts))
            }
            _ => {
                // Recovery: consume one token so we don't spin.
                self.next_token();
                None
            }
        }
    }
}
