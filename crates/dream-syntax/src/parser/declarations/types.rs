use super::super::Parser;
use crate::nodes::Type;
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {

    /// Parses a type alias: `type Name = ExistingType;`. The alias is recorded and resolved
    /// (erased) during `parse_type`, so it must be declared before use.
    pub(crate) fn parse_type_alias(&mut self) -> Result<(), Error> {
        self.match_token(TokenKind::TypeToken);
        let name = self.match_token(TokenKind::IdentifierToken);
        self.match_token(TokenKind::EqualToken);
        let aliased = self.parse_type()?;
        self.match_token(TokenKind::SemicolonToken);
        if self.type_aliases.contains_key(&name.text) {
            self.diagnostics.report_error(
                format!("Type alias '{}' is already defined", name.text),
                Some(name.position),
            );
        }
        self.type_aliases.insert(name.text, aliased);
        Ok(())
    }
    /// Parses a Type from the token stream, including array types
    pub(crate) fn parse_type(&mut self) -> Result<Type, Error> {
        // Function type: `fun(param, ...): ret` (the return annotation is optional, defaulting to
        // void). Used for first-class function values and function parameters.
        if self.current_token().kind == TokenKind::FunToken {
            self.match_token(TokenKind::FunToken);
            self.match_token(TokenKind::OpenParenthesisToken);
            let params = self.parse_delimited_list(TokenKind::CloseParenthesisToken, |p| {
                let is_ref = p.current_token().kind == TokenKind::RefToken;
                if is_ref {
                    p.match_token(TokenKind::RefToken);
                }
                let ty = p.parse_type()?;
                if is_ref {
                    let span = ty.get_span().unwrap_or_else(|| p.current_token().position);
                    Ok(Type::Struct(
                        SyntaxToken::new(
                            TokenKind::IdentifierToken,
                            span,
                            "RefBox".to_string(),
                        ),
                        Some(vec![ty]),
                    ))
                } else {
                    Ok(ty)
                }
            })?;
            let ret = if self.current_token().kind == TokenKind::ColonToken {
                self.match_token(TokenKind::ColonToken);
                self.parse_type()?
            } else {
                Type::Void
            };
            return Ok(Type::Function(params, Box::new(ret)));
        }

        let type_token = if self.current_token().kind == TokenKind::DataTypeToken {
            self.match_token(TokenKind::DataTypeToken)
        } else {
            self.match_token(TokenKind::IdentifierToken)
        };
        // `from_token` can fail to resolve a type token; route that through the diagnostics bag
        // (syntax's single error channel) and recover with a poison type so parsing continues,
        // rather than fabricating an `io::Error` that aborts the whole parse.
        let type_position = type_token.position;
        let mut parsed_type = match Type::from_token(type_token) {
            Ok(t) => t,
            Err(e) => {
                self.diagnostics
                    .report_error(e.to_string(), Some(type_position));
                Type::Unknown
            }
        };

        // Resolve a type alias to its underlying type (unless generic args follow). The array
        // suffix below still applies to the resolved type.
        if let Type::Struct(token, None) = &parsed_type {
            if self.current_token().kind != TokenKind::SmallerThanToken {
                if let Some(alias) = self.type_aliases.get(&token.text) {
                    parsed_type = alias.clone();
                }
            }
        }

        // Check for generic arguments
        if let Type::Struct(token, _) = &parsed_type {
            if self.current_token().kind == TokenKind::SmallerThanToken {
                self.match_token(TokenKind::SmallerThanToken);
                let args = self.parse_generic_args()?;
                parsed_type = Type::Struct(token.clone(), Some(args));
            }
        }

        // Check for array suffix `[]`
        while self.current_token().kind == TokenKind::OpenBracketToken {
            self.match_token(TokenKind::OpenBracketToken);
            self.match_token(TokenKind::CloseBracketToken);
            parsed_type = Type::Array(Box::new(parsed_type));
        }

        Ok(parsed_type)
    }
}
