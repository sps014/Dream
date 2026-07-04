use super::Parser;
use crate::lexer::Lexer;
use crate::nodes::{ExpressionNode, PatternNode, SwitchArm, SwitchArmBody, Type};
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use crate::token::token_kind::TokenKind::{EndOfFileToken, IdentifierToken};
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {
    /// Parses an expression with operator precedence
    pub(super) fn parse_expression(
        &mut self,
        parent_precedence: i32,
    ) -> Result<ExpressionNode<'a>, Error> {
        let mut left;
        let unary_precedence = self.current_token().kind.get_unary_precedence();
        if self.current_token().kind == TokenKind::AwaitToken {
            // `await <primary>` binds tightly to its operand so `await f() + 1` is `(await f()) + 1`.
            self.match_token(TokenKind::AwaitToken);
            let operand = self.parse_primary_expression()?;
            left = ExpressionNode::Await(self.arena.alloc(operand));
        } else if unary_precedence != 0 && unary_precedence >= parent_precedence {
            let operator_token = self.next_token();
            let operand = self.parse_expression(unary_precedence)?;
            left = ExpressionNode::Unary(operator_token, self.arena.alloc(operand));
        } else {
            left = self.parse_primary_expression()?;
        }
        loop {
            let precedence = self.current_token().kind.get_binary_precedence();
            if precedence == 0 || precedence <= parent_precedence {
                break;
            }

            let operator_token = self.next_token();
            if operator_token.kind == TokenKind::IsToken {
                let right_type = self.parse_type()?;
                // Optional `is`-with-binding: `expr is Type name` binds a narrowed local `name`.
                let binding = if self.current_token().kind == TokenKind::IdentifierToken {
                    Some(self.next_token())
                } else {
                    None
                };
                left = ExpressionNode::IsExpression(self.arena.alloc(left), right_type, binding);
            } else {
                let right = self.parse_expression(precedence)?;
                left = ExpressionNode::Binary(
                    self.arena.alloc(left),
                    operator_token,
                    self.arena.alloc(right),
                );
            }
        }

        // Ternary `cond ? a : b` binds looser than any binary operator and is right-associative.
        // It is only recognized at the top of an expression (parent_precedence == 0) so operands
        // of binary operators do not greedily consume a trailing `?`.
        if parent_precedence == 0 && self.current_token().kind == TokenKind::QuestionMarkToken {
            self.match_token(TokenKind::QuestionMarkToken);
            let then_expr = self.parse_expression(0)?;
            self.match_token(TokenKind::ColonToken);
            let else_expr = self.parse_expression(0)?;
            left = ExpressionNode::Ternary(
                self.arena.alloc(left),
                self.arena.alloc(then_expr),
                self.arena.alloc(else_expr),
            );
        }

        Ok(left)
    }
    /// Parses a primary expression (literal, identifier, parenthesized expression, or function call)
    pub(super) fn parse_primary_expression(&mut self) -> Result<ExpressionNode<'a>, Error> {
        // `switch (subject) { pattern => body, ... }` in expression (pattern-matching) form.
        if self.current_token().kind == TokenKind::SwitchToken {
            return self.parse_switch_expr();
        }
        //parse parenthesized expressions or cast
        if self.current_token().kind == TokenKind::OpenParenthesisToken {
            return self.parse_paren_or_cast();
        } else if self.current_token().kind == TokenKind::OpenBracketToken {
            // Array literal
            self.match_token(TokenKind::OpenBracketToken);
            let elements =
                self.parse_delimited_list(TokenKind::CloseBracketToken, |p| p.parse_expression(0))?;
            return Ok(ExpressionNode::ArrayLiteral(elements));
        } else if self.current_token().kind == TokenKind::BooleanToken {
            return Ok(ExpressionNode::Literal(Type::Boolean(
                self.match_token(TokenKind::BooleanToken),
            )));
        } else if self.current_token().kind == TokenKind::NullToken {
            self.match_token(TokenKind::NullToken);
            // `Nullable(Void)` represents the `null` literal until its concrete type is known.
            return Ok(ExpressionNode::Literal(Type::Nullable(Box::new(
                Type::Void,
            ))));
        }
        // A primitive type name used as a static-call receiver, e.g. `int.parse("5")`. The
        // keyword is treated as an identifier so the member/method-access loop below applies;
        // static dispatch is resolved later by the analyzer/codegen.
        else if self.current_token().kind == TokenKind::DataTypeToken
            && self.peek_token(1).kind == TokenKind::DotToken
        {
            // A primitive type name used as a static-call receiver only supports `.member`
            // access (no index suffix), so the dot-chain is parsed directly rather than via
            // the full postfix chain.
            let mut expr = ExpressionNode::Identifier(self.next_token());
            while self.current_token().kind == TokenKind::DotToken {
                expr = self.parse_member_access_step(expr)?;
            }
            return Ok(expr);
        }
        //parse identifiers
        else if self.current_token().kind == IdentifierToken {
            let mut is_invocation = false;
            // `Cache<int>.make(...)`: a generic *class* named as a static-call receiver, where the
            // type arguments belong to the class (not the method). Distinguished from a generic
            // constructor call (`Test<int>(...)`) by a `.` — rather than `(` — after the `<...>`.
            let mut is_generic_static = false;

            if self.peek_token(1).kind == TokenKind::OpenParenthesisToken {
                is_invocation = true;
            } else if self.peek_token(1).kind == TokenKind::SmallerThanToken {
                // Generic invocation like `Test<int>(...)`, tracking generic nesting so
                // `make<Pair<Box<int>, int>>(...)` is recognized as a call.
                if let Some(after) = self.scan_generic_args(2) {
                    match self.peek_token(after).kind {
                        TokenKind::OpenParenthesisToken => is_invocation = true,
                        TokenKind::DotToken => is_generic_static = true,
                        _ => {}
                    }
                }
            }

            if is_invocation {
                // A call on a bare identifier (free function or constructor, e.g.
                // `HttpClient(url)`) can still be the base of a postfix chain like
                // `HttpClient(url).set_header(...)` or `make().field`.
                let expr = self.parse_invocation_expression()?;
                return self.parse_postfix_chain(expr);
            } else if is_generic_static {
                let receiver = self.next_token();
                self.match_token(TokenKind::SmallerThanToken);
                let class_args = self.parse_generic_args()?;
                let expr = self.parse_generic_static_step(receiver, class_args)?;
                return self.parse_postfix_chain(expr);
            } else {
                // A bare identifier may be followed by an index/member/method postfix chain.
                let expr = ExpressionNode::Identifier(self.next_token());
                return self.parse_postfix_chain(expr);
            }
        } else if self.current_token().kind == TokenKind::NumberToken {
            let token = self.next_token();
            return Ok(ExpressionNode::Literal(Self::classify_number_literal(
                token,
            )));
        } else if self.current_token().kind == TokenKind::StringToken {
            return Ok(ExpressionNode::Literal(Type::String(self.next_token())));
        } else if self.current_token().kind == TokenKind::InterpolatedStringToken {
            let tok = self.next_token();
            return self.parse_interpolated_string(tok);
        } else if self.current_token().kind == TokenKind::CharToken {
            // A char literal `'a'` is a `char` whose backing token text is the (ASCII/code point)
            // value, so codegen can emit `i32.const <value>`. Escapes like '\n', '\t', '\\', '\''
            // and '\0' are supported.
            let tok = self.next_token();
            let value = Self::char_literal_value(&tok.text);
            let char_token =
                SyntaxToken::new(TokenKind::CharToken, tok.position, value.to_string());
            return Ok(ExpressionNode::Literal(Type::Char(char_token)));
        }

        let cur = self.current_token();
        if cur.kind != TokenKind::IdentifierToken {
            self.diagnostics.report_error(
                format!("Expected expression but found {:?}", cur.kind),
                Some(cur.position),
            );
            self.next_token(); // skip the unexpected token to avoid infinite loop
            return Ok(ExpressionNode::Identifier(SyntaxToken::new(
                TokenKind::IdentifierToken,
                cur.position,
                "".to_string(),
            )));
        }

        let identifier = self.match_token(TokenKind::IdentifierToken);
        Ok(ExpressionNode::Identifier(identifier))
    }
    /// Disambiguates a leading `(` between a cast (`(Type)expr`) and a parenthesized expression
    /// (`(expr)`), assuming the cursor is on the `(`. A cast is recognized when the parenthesized
    /// content is a type name (`(int)`, `(Node)`, `(Foo[])`) immediately followed by an
    /// expression-starting token. Parenthesized expressions allow a postfix chain so method calls
    /// on literals work (e.g. `(7).hash_code()`, `(arr)[0]`).
    fn parse_paren_or_cast(&mut self) -> Result<ExpressionNode<'a>, Error> {
        let is_cast = if self.peek_token(1).kind == TokenKind::DataTypeToken {
            true
        } else if self.peek_token(1).kind == TokenKind::IdentifierToken {
            // Could be `(Node)0` or `(x) + 1`
            // Let's check token after `)`
            let mut i = 2;
            // Skip a generic argument list so `(Container<int>)b` (and nested forms like
            // `(Pair<Box<int>, int>)x`) are recognized as casts. `scan_generic_args` tracks `<`/`>`
            // nesting (treating `>>` as two closes) and returns the peek offset after the matching
            // close; `None` means it is not a balanced generic list, so this is not a cast.
            let generic_ok = if self.peek_token(i).kind == TokenKind::SmallerThanToken {
                match self.scan_generic_args(i + 1) {
                    Some(after) => {
                        i = after;
                        true
                    }
                    None => false,
                }
            } else {
                true
            };
            if !generic_ok {
                false
            } else {
                while self.peek_token(i).kind == TokenKind::OpenBracketToken {
                    i += 2; // skip `[` and `]`
                }
                if self.peek_token(i).kind == TokenKind::CloseParenthesisToken {
                    let next_kind = self.peek_token(i + 1).kind;
                    // If the token after `)` is an expression starter, it's a cast
                    matches!(
                        next_kind,
                        TokenKind::NumberToken
                            | TokenKind::StringToken
                            | TokenKind::BooleanToken
                            | TokenKind::IdentifierToken
                            | TokenKind::OpenParenthesisToken
                            | TokenKind::OpenBracketToken
                            | TokenKind::MinusToken
                            | TokenKind::BangToken
                    )
                } else {
                    false
                }
            }
        } else {
            false
        };

        if is_cast {
            self.match_token(TokenKind::OpenParenthesisToken);
            let cast_type = self.parse_type()?;
            self.match_token(TokenKind::CloseParenthesisToken);
            let expression = self.parse_primary_expression()?;
            return Ok(ExpressionNode::Cast(
                cast_type,
                self.arena.alloc(expression),
            ));
        }

        //eat the open parenthesis
        self.match_token(TokenKind::OpenParenthesisToken);
        let expression = self.parse_expression(0)?;
        //eat the close parenthesis
        self.match_token(TokenKind::CloseParenthesisToken);
        // Allow postfix access on a parenthesized expression, e.g. `(7).hash_code()`,
        // `("x" + y).len()`, or `(arr)[0]`. This is required for method calls on literals
        // whose bare form would mis-lex (`7.hash_code()` reads `7.` as a float).
        let parenthesized = ExpressionNode::Parenthesized(self.arena.alloc(expression));
        self.parse_postfix_chain(parenthesized)
    }

    /// Continues parsing index (`[...]`) and member/method (`.name` / `.name(...)`) accesses onto an
    /// already-parsed base expression. Used so a call on a bare identifier (e.g. a constructor like
    /// `HttpClient(url)`) can be chained: `HttpClient(url).set_header(...)`.
    pub(super) fn parse_postfix_chain(
        &mut self,
        base: ExpressionNode<'a>,
    ) -> Result<ExpressionNode<'a>, Error> {
        let mut expr = base;
        loop {
            if self.current_token().kind == TokenKind::OpenBracketToken {
                self.match_token(TokenKind::OpenBracketToken);
                let index = self.parse_expression(0)?;
                self.match_token(TokenKind::CloseBracketToken);
                expr = ExpressionNode::IndexAccess(self.arena.alloc(expr), self.arena.alloc(index));
            } else if self.current_token().kind == TokenKind::DotToken {
                expr = self.parse_member_access_step(expr)?;
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Parses a single `.member` access step onto `base`, consuming the `.`, an optional method
    /// generic-argument list (`<...>` immediately followed by `(`), and—when a `(` follows—the
    /// call-argument list, producing a [`ExpressionNode::MethodCall`]; otherwise a plain
    /// [`ExpressionNode::MemberAccess`]. Shared by every dot/method site (postfix chain, bare
    /// identifier chain, and primitive static-call receiver).
    fn parse_member_access_step(
        &mut self,
        base: ExpressionNode<'a>,
    ) -> Result<ExpressionNode<'a>, Error> {
        self.match_token(TokenKind::DotToken);
        let member = self.match_member_name();

        let mut generic_args = None;
        if self.current_token().kind == TokenKind::SmallerThanToken {
            // Method generic args, e.g. `obj.cast<Foo<int>>()`. Only treat as generic when the
            // balanced `<...>` is immediately followed by `(`.
            let is_generic = self
                .scan_generic_args(1)
                .map(|after| self.peek_token(after).kind == TokenKind::OpenParenthesisToken)
                .unwrap_or(false);
            if is_generic {
                self.match_token(TokenKind::SmallerThanToken);
                generic_args = Some(self.parse_generic_args()?);
            }
        }

        if self.current_token().kind == TokenKind::OpenParenthesisToken {
            self.match_token(TokenKind::OpenParenthesisToken);
            let params = self.parse_delimited_list(TokenKind::CloseParenthesisToken, |p| {
                p.parse_expression(0)
            })?;
            Ok(ExpressionNode::MethodCall(
                self.arena.alloc(base),
                member,
                generic_args,
                params,
            ))
        } else {
            Ok(ExpressionNode::MemberAccess(self.arena.alloc(base), member))
        }
    }

    /// Parses the `.method(args)` (or bare `.member`) step of a generic-class static receiver whose
    /// type arguments (`class_args`) were parsed just before the `.`, e.g. the `.make(1)` in
    /// `Cache<int>.make(1)`. The class type arguments ride on the resulting [`MethodCall`]'s
    /// generic-argument slot; the analyzer interprets them as the receiver class's arguments (the
    /// class is monomorphized and the concrete static method dispatched).
    fn parse_generic_static_step(
        &mut self,
        receiver: SyntaxToken,
        class_args: Vec<Type>,
    ) -> Result<ExpressionNode<'a>, Error> {
        self.match_token(TokenKind::DotToken);
        let member = self.match_member_name();
        let base = ExpressionNode::Identifier(receiver);
        if self.current_token().kind == TokenKind::OpenParenthesisToken {
            self.match_token(TokenKind::OpenParenthesisToken);
            let params = self.parse_delimited_list(TokenKind::CloseParenthesisToken, |p| {
                p.parse_expression(0)
            })?;
            Ok(ExpressionNode::MethodCall(
                self.arena.alloc(base),
                member,
                Some(class_args),
                params,
            ))
        } else {
            Ok(ExpressionNode::MemberAccess(self.arena.alloc(base), member))
        }
    }

    /// Parses a function invocation expression
    pub(super) fn parse_invocation_expression(&mut self) -> Result<ExpressionNode<'a>, Error> {
        let function_name = self.match_token(TokenKind::IdentifierToken);

        let mut generic_arguments = None;
        if self.current_token().kind == TokenKind::SmallerThanToken {
            self.match_token(TokenKind::SmallerThanToken);
            generic_arguments = Some(self.parse_generic_args()?);
        }

        //eat the open parenthesis
        self.match_token(TokenKind::OpenParenthesisToken);
        let mut arguments = Vec::new();
        while self.current_token().kind != TokenKind::CloseParenthesisToken
            && self.current_token().kind != EndOfFileToken
        {
            let iter = self.current_token_index;
            //parse the argument
            let argument = self.parse_expression(0)?;
            arguments.push(argument);
            if self.current_token().kind == TokenKind::CommaToken
                && self.peek_token(1).kind != TokenKind::CloseParenthesisToken
            {
                //eat the comma
                self.match_token(TokenKind::CommaToken);
            }
            self.ensure_progress(iter);
        }
        //eat the close parenthesis
        self.match_token(TokenKind::CloseParenthesisToken);
        Ok(ExpressionNode::FunctionCall(
            function_name,
            generic_arguments,
            arguments,
        ))
    }

    /// Parses the shared `switch (subject) {` header, returning the subject. Both the
    /// pattern-matching form ([`parse_switch_expr`]) and the C-style `case`/`default` form
    /// ([`parse_switch`](Self::parse_switch)) start here, then branch on the body.
    pub(super) fn parse_switch_header(&mut self) -> Result<ExpressionNode<'a>, Error> {
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
    pub(super) fn parse_switch_arms(&mut self) -> Result<Vec<SwitchArm<'a>>, Error> {
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
    pub(super) fn parse_switch_expr(&mut self) -> Result<ExpressionNode<'a>, Error> {
        let subject = self.parse_switch_header()?;
        let arms = self.parse_switch_arms()?;
        Ok(ExpressionNode::Switch(self.arena.alloc(subject), arms))
    }

    /// Parses a single match pattern: `_` (wildcard), a literal, a bare identifier (a binding,
    /// later reinterpreted as a unit variant by the analyzer when it names one), or a variant
    /// pattern `Variant(sub, ...)` / `Enum.Variant(sub, ...)`.
    pub(super) fn parse_pattern(&mut self) -> Result<PatternNode, Error> {
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
            _ => Ok(PatternNode::Literal(self.parse_literal_pattern()?)),
        }
    }

    /// Parses the parenthesized sub-pattern list of a variant pattern, e.g. the `(x, None)` in
    /// `Pair(x, None)`. Returns an empty list when there is no `(...)`.
    fn parse_pattern_args(&mut self) -> Result<Vec<PatternNode>, Error> {
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
    pub(super) fn classify_number_literal(mut token: SyntaxToken) -> Type {
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

    /// Parses a literal used as a pattern (`0`, `-5`, `3.14`, `"s"`, `'c'`, `true`, `null`). Also
    /// reused to parse constant-literal default parameter values.
    pub(super) fn parse_literal_pattern(&mut self) -> Result<Type, Error> {
        let cur = self.current_token();
        match cur.kind {
            TokenKind::BooleanToken => Ok(Type::Boolean(self.match_token(TokenKind::BooleanToken))),
            TokenKind::StringToken => Ok(Type::String(self.match_token(TokenKind::StringToken))),
            TokenKind::NullToken => {
                self.match_token(TokenKind::NullToken);
                Ok(Type::Nullable(Box::new(Type::Void)))
            }
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

    /// Lowers an interpolated string literal `$"...{expr}..."` into the existing string
    /// concatenation chain. `$"{y+68} is {x}"` becomes `"" + (y + 68) + " is " + (x)`, reusing
    /// the analyzer/codegen `string + T` path that auto-converts each non-string operand through
    /// the `to_string` object protocol. The chain is seeded with an empty string literal so the
    /// whole expression is always typed `string`, even for a lone hole like `$"{x}"`.
    fn parse_interpolated_string(
        &mut self,
        token: SyntaxToken,
    ) -> Result<ExpressionNode<'a>, Error> {
        let pos = token.position;
        // Strip the leading `$"` and trailing `"`. The lexer guarantees this shape.
        let raw = token.text.as_str();
        let body = raw
            .strip_prefix("$\"")
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or("");

        // Byte offset (in the original file) of the first character of `body`: skip `$"`.
        let body_base = pos.start + 2;

        let chars: Vec<char> = body.chars().collect();
        let mut i = 0;
        // Byte offset of `chars[i]` within `body`, kept in lockstep with `i` so hole sources can be
        // mapped back to absolute file positions for IDE navigation.
        let mut byte_pos = 0usize;
        let mut text_buf = String::new();
        // Each segment is either literal text (`Ok`) or a hole `(source, byte offset in body)`.
        let mut segments: Vec<Result<String, (String, usize)>> = Vec::new();

        while i < chars.len() {
            let c = chars[i];
            if c == '{' {
                // `{{` is an escaped literal `{`.
                if i + 1 < chars.len() && chars[i + 1] == '{' {
                    text_buf.push('{');
                    byte_pos += 2;
                    i += 2;
                    continue;
                }
                // Open a hole: flush any pending literal text first.
                if !text_buf.is_empty() {
                    segments.push(Ok(std::mem::take(&mut text_buf)));
                }
                byte_pos += 1; // consume `{`
                i += 1;
                let hole_byte_start = byte_pos;
                let mut depth = 1;
                let mut hole = String::new();
                while i < chars.len() && depth > 0 {
                    let h = chars[i];
                    let advance = h.len_utf8();
                    if h == '{' {
                        depth += 1;
                        hole.push(h);
                    } else if h == '}' {
                        depth -= 1;
                        if depth == 0 {
                            byte_pos += advance; // consume the matching `}`
                            i += 1;
                            break;
                        }
                        hole.push(h);
                    } else {
                        hole.push(h);
                    }
                    byte_pos += advance;
                    i += 1;
                }
                if depth > 0 {
                    self.diagnostics.report_error(
                        "unterminated '{' in interpolated string".to_string(),
                        Some(pos),
                    );
                }
                segments.push(Err((hole, hole_byte_start)));
            } else if c == '}' {
                // `}}` is an escaped literal `}`.
                if i + 1 < chars.len() && chars[i + 1] == '}' {
                    text_buf.push('}');
                    byte_pos += 2;
                    i += 2;
                    continue;
                }
                self.diagnostics.report_error(
                    "unmatched '}' in interpolated string; use '}}' for a literal brace"
                        .to_string(),
                    Some(pos),
                );
                text_buf.push('}');
                byte_pos += 1;
                i += 1;
            } else {
                text_buf.push(c);
                byte_pos += c.len_utf8();
                i += 1;
            }
        }
        if !text_buf.is_empty() {
            segments.push(Ok(text_buf));
        }

        // Seed with an empty string literal so the result is always `string`.
        let mut acc = self.make_string_literal(String::new(), pos);
        for segment in segments {
            let right = match segment {
                Ok(text) => self.make_string_literal(text, pos),
                Err((hole, hole_byte_start)) => {
                    self.parse_interpolation_hole(hole, body_base + hole_byte_start, pos)?
                }
            };
            let left_ref = self.arena.alloc(acc);
            let right_ref = self.arena.alloc(right);
            let plus = SyntaxToken::new(TokenKind::PlusToken, pos, "+".to_string());
            acc = ExpressionNode::Binary(left_ref, plus, right_ref);
        }
        Ok(acc)
    }

    /// Builds a string literal AST node from raw (already-escaped) inner text by re-adding the
    /// surrounding quotes that codegen strips. Any backslash escapes carried over from the
    /// interpolated literal are preserved verbatim, matching plain string literals.
    fn make_string_literal(
        &self,
        text: String,
        pos: dream_text::text_span::TextSpan,
    ) -> ExpressionNode<'a> {
        let tok = SyntaxToken::new(TokenKind::StringToken, pos, format!("\"{}\"", text));
        ExpressionNode::Literal(Type::String(tok))
    }

    /// Parses the source of a single `{...}` hole into an expression using a child parser that
    /// shares this parser's arena and diagnostics (so allocated nodes live in the same arena and
    /// errors surface on the same bag). `abs_offset` is the byte position of the hole's first
    /// character in the original file; sub-token spans are remapped to absolute file coordinates so
    /// IDE features (hover, go-to-definition, references) resolve correctly inside `{holes}`.
    fn parse_interpolation_hole(
        &mut self,
        source: String,
        abs_offset: usize,
        pos: dream_text::text_span::TextSpan,
    ) -> Result<ExpressionNode<'a>, Error> {
        if source.trim().is_empty() {
            self.diagnostics.report_error(
                "empty '{}' interpolation hole in string".to_string(),
                Some(pos),
            );
            return Ok(self.make_string_literal(String::new(), pos));
        }

        let parent_line_text = self.lexer.line_text();
        let mut lexer = Lexer::new(source);
        let mut tokens = lexer.lex_all(self.diagnostics);
        // Translate hole-relative byte spans to absolute file positions.
        for token in tokens.iter_mut() {
            token.position = dream_text::text_span::TextSpan::new(
                (
                    abs_offset + token.position.start,
                    abs_offset + token.position.end,
                ),
                &parent_line_text,
            );
        }
        let mut sub: Parser<'a, '_> = Parser {
            lexer,
            tokens,
            current_token_index: 0,
            arena: self.arena,
            diagnostics: &mut *self.diagnostics,
            foreach_counter: 0,
            type_aliases: self.type_aliases.clone(),
        };
        let expr = sub.parse_expression(0)?;
        if sub.current_token().kind != EndOfFileToken {
            let extra = sub.current_token();
            sub.diagnostics.report_error(
                format!(
                    "unexpected {} after expression in interpolation hole",
                    extra.kind.friendly_name()
                ),
                Some(pos),
            );
        }
        Ok(expr)
    }
}
