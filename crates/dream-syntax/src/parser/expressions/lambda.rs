use super::super::Parser;
use crate::nodes::{
    ExpressionNode, LambdaBody, LambdaNode, ParameterNode,
    Type,
};
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {

    /// Lookahead for an arrow-lambda literal (`(params) => ...`), starting at peek offset `start`
    /// True when peek offset `start` begins `<…>(…) =>` — a generic arrow-lambda.
    pub(crate) fn is_generic_lambda_start_at(&self, start: usize) -> bool {
        if self.peek_token(start).kind != TokenKind::SmallerThanToken {
            return false;
        }
        let Some(after_generics) = self.scan_generic_args(start + 1) else {
            return false;
        };
        if self.peek_token(after_generics).kind != TokenKind::OpenParenthesisToken {
            return false;
        }
        self.is_lambda_start_at(after_generics)
    }

    /// which must point at the leading `(`. Finds the matching `)` (tracking paren nesting only —
    /// a lambda's parameter types cannot themselves contain unbalanced parens) and checks whether
    /// it's immediately followed by `=>`. Safe to try *before* the cast/paren dispatch: `) =>`
    /// never validly follows an existing cast (`(Type)expr`) or a parenthesized expression's
    /// postfix chain (`(expr).member`, `(expr)[i]`, `(expr)?`).
    pub(crate) fn is_lambda_start_at(&self, start: usize) -> bool {
        let mut depth = 0i32;
        let mut i = start;
        loop {
            match self.peek_token(i).kind {
                TokenKind::OpenParenthesisToken => depth += 1,
                TokenKind::CloseParenthesisToken => {
                    depth -= 1;
                    if depth == 0 {
                        return self.peek_token(i + 1).kind == TokenKind::FatArrowToken;
                    }
                }
                TokenKind::EndOfFileToken => return false,
                _ => {}
            }
            i += 1;
        }
    }

    /// Parses an arrow-lambda literal `(params) => expr` / `(params) => { stmts }`, optionally
    /// preceded by `<TypeParams>`. `is_async` is true when the caller already consumed a leading
    /// `async`.
    pub(crate) fn parse_lambda(&mut self, is_async: bool) -> Result<ExpressionNode<'a>, Error> {
        let (generic_parameters, generic_constraints) = self.take_generic_params();
        let open_paren_position = self.current_token().position;
        let parameters = self.parse_lambda_parameters()?;
        self.match_token(TokenKind::FatArrowToken);
        let body = if self.current_token().kind == TokenKind::CurlyOpenBracketToken {
            LambdaBody::Block(self.parse_block()?)
        } else {
            let expr = self.parse_expression(0)?;
            LambdaBody::Expr(self.arena.alloc(expr))
        };
        Ok(ExpressionNode::Lambda(self.arena.alloc(LambdaNode {
            open_paren_position,
            is_async,
            generic_parameters,
            generic_constraints,
            parameters,
            body,
        })))
    }

    /// Parses a lambda's parameter list, `(name [: Type] [= default], ...)`. Identical to
    /// [`parse_formal_parameters`](Self::parse_formal_parameters) except the `: Type` annotation is
    /// optional: an omitted type is recorded as `Type::Unknown`, a sentinel the analyzer resolves
    /// from the lambda's expected `fun(...)` context (or reports a diagnostic for if none exists).
    pub(crate) fn parse_lambda_parameters(&mut self) -> Result<Vec<ParameterNode>, Error> {
        let mut params = vec![];
        self.match_token(TokenKind::OpenParenthesisToken);

        let mut seen_default = false;
        while self.current_token().kind != TokenKind::CloseParenthesisToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let index_before = self.current_token_index;

            // A `ref name: T` lambda parameter, mirroring named-function ref parameters.
            let is_ref = self.current_token().kind == TokenKind::RefToken;
            if is_ref {
                self.match_token(TokenKind::RefToken);
            }

            let param = self.match_token(TokenKind::IdentifierToken);

            let param_type = if self.current_token().kind == TokenKind::ColonToken {
                self.match_token(TokenKind::ColonToken);
                self.parse_type()?
            } else {
                Type::Unknown
            };

            if is_ref {
                if self.current_token().kind == TokenKind::EqualToken {
                    self.diagnostics.report_error(
                        format!(
                            "'ref' parameter '{}' cannot have a default value",
                            param.text
                        ),
                        Some(param.position),
                    );
                }
                params.push(ParameterNode::by_ref(param, param_type));
            } else {
                let default = if self.current_token().kind == TokenKind::EqualToken {
                    self.match_token(TokenKind::EqualToken);
                    seen_default = true;
                    Some(self.parse_literal_pattern()?)
                } else {
                    if seen_default {
                        self.diagnostics.report_error(
                            format!(
                                "required parameter '{}' cannot follow a parameter with a default value",
                                param.text
                            ),
                            Some(param.position),
                        );
                    }
                    None
                };
                params.push(ParameterNode::with_default(param, param_type, default));
            }

            if self.current_token_index == index_before {
                self.next_token();
            }
            if self.current_token().kind == TokenKind::CommaToken
                && matches!(
                    self.peek_token(1).kind,
                    TokenKind::IdentifierToken | TokenKind::RefToken
                )
            {
                self.match_token(TokenKind::CommaToken);
            }
        }

        self.match_token(TokenKind::CloseParenthesisToken);
        Ok(params)
    }
}
