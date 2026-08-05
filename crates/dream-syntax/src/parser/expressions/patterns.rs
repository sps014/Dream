use super::super::Parser;
use crate::nodes::{
    ExpressionNode, PatternNode, SwitchArm, SwitchArmBody,
    Type,
};
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {

    /// Parses the shared `switch (subject) {` header, returning the subject. Both the
    /// pattern-matching form ([`parse_switch_expr`]) and the C-style `case`/`default` form
    /// ([`parse_switch`](Self::parse_switch)) start here, then branch on the body.
    pub(crate) fn parse_switch_header(&mut self) -> Result<ExpressionNode<'a>, Error> {
        self.match_token(TokenKind::SwitchToken);
        self.match_token(TokenKind::OpenParenthesisToken);
        let subject = self.parse_expression(0)?;
        self.match_token(TokenKind::CloseParenthesisToken);
        self.match_token(TokenKind::CurlyOpenBracketToken);
        Ok(subject)
    }

    /// Parses the pattern-matching arms `pattern [if guard] => body, ...` up to and including the
    /// closing `}`, assuming the `switch (...) {` header has already been consumed. Each arm body is
    /// either an expression (`=> expr`) or a statement block (`=> { ... }`); a trailing comma after
    /// an arm is optional.
    pub(crate) fn parse_switch_arms(&mut self) -> Result<Vec<SwitchArm<'a>>, Error> {
        self.parse_delimited_list(TokenKind::CurlyCloseBracketToken, |p| {
            let pattern = p.parse_pattern()?;

            // Optional `if <guard>` after the pattern.
            let guard = if p.current_token().kind == TokenKind::IfToken {
                p.match_token(TokenKind::IfToken);
                Some(p.parse_expression(0)?)
            } else {
                None
            };

            p.match_token(TokenKind::FatArrowToken);

            let body = if p.current_token().kind == TokenKind::CurlyOpenBracketToken {
                SwitchArmBody::Block(p.parse_block()?)
            } else {
                SwitchArmBody::Expr(p.parse_expression(0)?)
            };

            Ok(SwitchArm {
                pattern,
                guard,
                body,
            })
        })
    }

    /// Parses a `switch (subject) { pattern [if guard] => body, ... }` expression (the
    /// pattern-matching form). The C-style `case`/`default` form is a statement and is parsed by
    /// [`parse_switch`](Self::parse_switch).
    pub(crate) fn parse_switch_expr(&mut self) -> Result<ExpressionNode<'a>, Error> {
        let subject = self.parse_switch_header()?;
        let arms = self.parse_switch_arms()?;
        Ok(ExpressionNode::Switch(self.arena.alloc(subject), arms))
    }

    /// Parses a match pattern, including a top-level or-pattern (`pat | pat | ...`): one or more
    /// [`Self::parse_pattern_atom`]s separated by `|`, collapsing to a single [`PatternNode::Or`]
    /// only when there is more than one alternative.
    pub(crate) fn parse_pattern(&mut self) -> Result<PatternNode, Error> {
        let first = self.parse_pattern_atom()?;
        if self.current_token().kind != TokenKind::BitWisePipeToken {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.current_token().kind == TokenKind::BitWisePipeToken {
            self.match_token(TokenKind::BitWisePipeToken);
            alts.push(self.parse_pattern_atom()?);
        }
        Ok(PatternNode::Or(alts))
    }

    /// Parses a single pattern "atom" (one or-pattern alternative): `_` (wildcard), a literal or
    /// inclusive range literal (`1..5`), a bare identifier (a binding, later reinterpreted as a
    /// unit variant by the analyzer when it names one), or a variant pattern `Variant(sub, ...)` /
    /// `Enum.Variant(sub, ...)`.
    pub(crate) fn parse_pattern_atom(&mut self) -> Result<PatternNode, Error> {
        let cur = self.current_token();
        match cur.kind {
            TokenKind::IdentifierToken => {
                if cur.text == "_" {
                    let tok = self.next_token();
                    return Ok(PatternNode::Wildcard(tok));
                }
                let first = self.match_token(TokenKind::IdentifierToken);
                // `Enum.Variant[(...)]` - a qualified variant pattern.
                if self.current_token().kind == TokenKind::DotToken {
                    self.match_token(TokenKind::DotToken);
                    let variant = self.match_token(TokenKind::IdentifierToken);
                    let subs = self.parse_pattern_args()?;
                    return Ok(PatternNode::Variant(Some(first), variant, subs));
                }
                // `Variant(...)` - an unqualified variant pattern with a payload.
                if self.current_token().kind == TokenKind::OpenParenthesisToken {
                    let subs = self.parse_pattern_args()?;
                    return Ok(PatternNode::Variant(None, first, subs));
                }
                // A bare identifier: a binding (or a unit variant, resolved during analysis).
                Ok(PatternNode::Binding(first))
            }
            _ => {
                let lo = self.parse_literal_pattern()?;
                if self.current_token().kind == TokenKind::DotDotToken {
                    self.match_token(TokenKind::DotDotToken);
                    let hi = self.parse_literal_pattern()?;
                    return Ok(PatternNode::Range(lo, hi));
                }
                Ok(PatternNode::Literal(lo))
            }
        }
    }

    /// Parses the parenthesized sub-pattern list of a variant pattern, e.g. the `(x, None)` in
    /// `Pair(x, None)`. Returns an empty list when there is no `(...)`.
    pub(crate) fn parse_pattern_args(&mut self) -> Result<Vec<PatternNode>, Error> {
        let mut subs = Vec::new();
        if self.current_token().kind == TokenKind::OpenParenthesisToken {
            self.match_token(TokenKind::OpenParenthesisToken);
            subs =
                self.parse_delimited_list(TokenKind::CloseParenthesisToken, |p| p.parse_pattern())?;
        }
        Ok(subs)
    }

    /// Classifies a `NumberToken` into its concrete numeric [`Type`], stripping any type suffix
    /// from the token's text so downstream stages see only the numeric value. Recognized suffixes
    /// (case-insensitive): `f` (float), `d` (double), `L` (long), `u` (uint), `uL`/`Lu` (ulong),
    /// `b` (byte). A bare literal with a decimal point is `float`, otherwise `int`.
    pub(crate) fn classify_number_literal(mut token: SyntaxToken) -> Type {
        let text = token.text.clone();
        let num_end = text
            .find(|c: char| c.is_ascii_alphabetic())
            .unwrap_or(text.len());
        let (num, suffix) = text.split_at(num_end);
        token.text = num.to_string();
        match suffix.to_ascii_lowercase().as_str() {
            "b" => Type::Byte(token),
            "ul" | "lu" => Type::ULong(token),
            "l" => Type::Long(token),
            "u" => Type::UInt(token),
            "d" => Type::Double(token),
            "f" => Type::Float(token),
            _ => {
                if num.contains('.') {
                    Type::Float(token)
                } else {
                    Type::Integer(token)
                }
            }
        }
    }

    /// Parses a literal used as a pattern (`0`, `-5`, `3.14`, `"s"`, `'c'`, `true`). Also
    /// reused to parse constant-literal default parameter values.
    pub(crate) fn parse_literal_pattern(&mut self) -> Result<Type, Error> {
        let cur = self.current_token();
        match cur.kind {
            TokenKind::BooleanToken => Ok(Type::Boolean(self.match_token(TokenKind::BooleanToken))),
            TokenKind::StringToken => Ok(Type::String(self.match_token(TokenKind::StringToken))),
            TokenKind::CharToken => {
                let tok = self.next_token();
                let value = Self::char_literal_value(&tok.text);
                let char_token =
                    SyntaxToken::new(TokenKind::CharToken, tok.position, value.to_string());
                Ok(Type::Char(char_token))
            }
            TokenKind::MinusToken | TokenKind::NumberToken => {
                let negative = cur.kind == TokenKind::MinusToken;
                if negative {
                    self.match_token(TokenKind::MinusToken);
                }
                let token = self.match_token(TokenKind::NumberToken);
                let mut classified = Self::classify_number_literal(token);
                if negative {
                    // Prepend the sign to the (suffix-stripped) numeric text of the literal.
                    classified = match classified {
                        Type::Integer(mut t) => {
                            t.text = format!("-{}", t.text);
                            Type::Integer(t)
                        }
                        Type::Long(mut t) => {
                            t.text = format!("-{}", t.text);
                            Type::Long(t)
                        }
                        Type::Float(mut t) => {
                            t.text = format!("-{}", t.text);
                            Type::Float(t)
                        }
                        Type::Double(mut t) => {
                            t.text = format!("-{}", t.text);
                            Type::Double(t)
                        }
                        other => other,
                    };
                }
                Ok(classified)
            }
            _ => {
                self.diagnostics.report_error(
                    format!("Expected a pattern but found {}", cur.kind.friendly_name()),
                    Some(cur.position),
                );
                self.next_token();
                Ok(Type::Unknown)
            }
        }
    }
}
