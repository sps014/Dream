use super::super::Parser;
use crate::nodes::{
    ExpressionNode,
    Type,
};
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use crate::token::token_kind::TokenKind::{EndOfFileToken, IdentifierToken};
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {
    /// Parses an expression with operator precedence
    pub(crate) fn parse_expression(
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
    pub(crate) fn parse_primary_expression(&mut self) -> Result<ExpressionNode<'a>, Error> {
        // `switch (subject) { pattern => body, ... }` in expression (pattern-matching) form.
        if self.current_token().kind == TokenKind::SwitchToken {
            return self.parse_switch_expr();
        }
        // `async <T>(params) => …` / `async (params) => …`
        if self.current_token().kind == TokenKind::AsyncToken {
            if self.peek_token(1).kind == TokenKind::OpenParenthesisToken
                && self.is_lambda_start_at(1)
            {
                self.match_token(TokenKind::AsyncToken);
                return self.parse_lambda(true);
            }
            if self.peek_token(1).kind == TokenKind::SmallerThanToken
                && self.is_generic_lambda_start_at(1)
            {
                self.match_token(TokenKind::AsyncToken);
                return self.parse_lambda(true);
            }
        }
        // `<T>(params) => …` generic arrow-lambda
        if self.current_token().kind == TokenKind::SmallerThanToken
            && self.is_generic_lambda_start_at(0)
        {
            return self.parse_lambda(false);
        }
        //parse parenthesized expressions, casts, or arrow-lambdas
        if self.current_token().kind == TokenKind::OpenParenthesisToken {
            if self.is_lambda_start_at(0) {
                return self.parse_lambda(false);
            }
            return self.parse_paren_or_cast();
        } else if self.current_token().kind == TokenKind::OpenBracketToken {
            // Array literal
            self.match_token(TokenKind::OpenBracketToken);
            let elements =
                self.parse_delimited_list(TokenKind::CloseBracketToken, |p| p.parse_expression(0))?;
            return Ok(ExpressionNode::ArrayLiteral(elements));
        } else if self.current_token().kind == TokenKind::CurlyOpenBracketToken {
            return self.parse_set_or_map_literal();
        } else if self.current_token().kind == TokenKind::BooleanToken {
            return Ok(ExpressionNode::Literal(Type::Boolean(
                self.match_token(TokenKind::BooleanToken),
            )));
        }
        // A primitive type name used as a static-call receiver, e.g. `int.parse("5")`. The
        // keyword is treated as an identifier so the member/method-access loop below applies;
        // static dispatch is resolved later by the analyzer. After the `.` chain, the full
        // postfix chain applies (`[…]`, further `.member`, try `?`).
        else if self.current_token().kind == TokenKind::DataTypeToken
            && self.peek_token(1).kind == TokenKind::DotToken
        {
            let mut expr = ExpressionNode::Identifier(self.next_token());
            while self.current_token().kind == TokenKind::DotToken {
                expr = self.parse_member_access_step(expr)?;
            }
            return self.parse_postfix_chain(expr);
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
    pub(crate) fn parse_paren_or_cast(&mut self) -> Result<ExpressionNode<'a>, Error> {
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
                    // If the token after `)` is an expression starter, it's a cast — except for
                    // `(ident)(…)` when `ident` is neither a primitive spelling nor a PascalCase
                    // nominal type: that shape is the postfix-call form `(fun_value)(args)`.
                    // Primitive casts like `(long)((int)c)` and nominal casts like `(Node)(x)`
                    // still win; `(f)(2)` becomes Parenthesized + Call.
                    if next_kind == TokenKind::OpenParenthesisToken {
                        let type_name = &self.peek_token(1).text;
                        crate::nodes::types::PRIMITIVE_TYPE_NAMES.contains(&type_name.as_str())
                            || type_name
                                .chars()
                                .next()
                                .is_some_and(|c| c.is_uppercase())
                    } else {
                        matches!(
                            next_kind,
                            TokenKind::NumberToken
                                | TokenKind::StringToken
                                | TokenKind::BooleanToken
                                | TokenKind::IdentifierToken
                                | TokenKind::OpenBracketToken
                                | TokenKind::MinusToken
                                | TokenKind::BangToken
                        )
                    }
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

    /// Continues parsing index (`[...]`), call (`(...)`), and member/method (`.name` / `.name(...)`)
    /// accesses onto an already-parsed base expression. Used so a call on a bare identifier (e.g. a
    /// constructor like `HttpClient(url)`) can be chained: `HttpClient(url).set_header(...)`, and so
    /// a `fun(...)` value expression can be invoked: `make()()`, `(f)(x)`.
    pub(crate) fn parse_postfix_chain(
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
            } else if self.current_token().kind == TokenKind::OpenParenthesisToken {
                // Generic type args on a postfix call (`expr<T>(...)`) are not supported; only
                // bare `expr(...)`. Named free-function generics stay on the `FunctionCall` path.
                self.match_token(TokenKind::OpenParenthesisToken);
                let mut arguments = Vec::new();
                while self.current_token().kind != TokenKind::CloseParenthesisToken
                    && self.current_token().kind != EndOfFileToken
                {
                    let iter = self.current_token_index;
                    arguments.push(self.parse_call_argument()?);
                    if self.current_token().kind == TokenKind::CommaToken
                        && self.peek_token(1).kind != TokenKind::CloseParenthesisToken
                    {
                        self.match_token(TokenKind::CommaToken);
                    }
                    self.ensure_progress(iter);
                }
                self.match_token(TokenKind::CloseParenthesisToken);
                expr = ExpressionNode::Call(self.arena.alloc(expr), None, arguments);
            } else if self.current_token().kind == TokenKind::DotToken {
                expr = self.parse_member_access_step(expr)?;
            } else if self.current_token().kind == TokenKind::QuestionMarkToken
                && self.is_try_propagation_question_mark()
            {
                self.match_token(TokenKind::QuestionMarkToken);
                expr = ExpressionNode::Try(self.arena.alloc(expr));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Disambiguates a postfix `?` (try-propagation, `expr?`) from the leading `?` of a ternary
    /// (`cond ? a : b`). Prefers try-propagation unless a matching ternary `:` appears at nesting
    /// depth 0 in the following tokens (so `half(n)? + 1` and `if (half(n)? > 0)` parse as try,
    /// while `cond ? a : b` stays ternary). An immediately following `?` is always try, so
    /// `x? ? a : b` is `(x?) ? a : b`.
    pub(crate) fn is_try_propagation_question_mark(&self) -> bool {
        if self.peek_token(1).kind == TokenKind::QuestionMarkToken {
            return true;
        }
        let mut depth: i32 = 0;
        let mut i = 1;
        loop {
            let kind = self.peek_token(i).kind;
            match kind {
                TokenKind::EndOfFileToken | TokenKind::SemicolonToken => return true,
                TokenKind::CommaToken if depth == 0 => return true,
                TokenKind::ColonToken if depth == 0 => return false,
                TokenKind::OpenParenthesisToken
                | TokenKind::OpenBracketToken
                | TokenKind::CurlyOpenBracketToken => depth += 1,
                TokenKind::CloseParenthesisToken
                | TokenKind::CloseBracketToken
                | TokenKind::CurlyCloseBracketToken => {
                    depth -= 1;
                    if depth < 0 {
                        return true;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Parses one call argument: either a plain expression, or a named argument `name: value`
    /// (recognized by a `identifier ':'` lookahead, which never otherwise starts an expression —
    /// a bare `?`, not `:`, introduces a ternary's branches). Shared by every call-argument list
    /// (free/generic function calls, method calls, generic-static-receiver calls) so named-argument
    /// support only has to be taught once.
    pub(crate) fn parse_call_argument(&mut self) -> Result<ExpressionNode<'a>, Error> {
        if self.current_token().kind == IdentifierToken
            && self.peek_token(1).kind == TokenKind::ColonToken
        {
            let name = self.next_token();
            self.match_token(TokenKind::ColonToken);
            let value = self.parse_expression(0)?;
            return Ok(ExpressionNode::NamedArg(name, self.arena.alloc(value)));
        }
        // A pass-by-reference argument: `ref place`. The place shape (identifier/member/index) is
        // validated by the analyzer, not here — parsing accepts any unary-level expression.
        if self.current_token().kind == TokenKind::RefToken {
            self.match_token(TokenKind::RefToken);
            let value = self.parse_expression(0)?;
            return Ok(ExpressionNode::RefArgument(self.arena.alloc(value)));
        }
        self.parse_expression(0)
    }

    /// Parses a single `.member` access step onto `base`, consuming the `.`, an optional method
    /// generic-argument list (`<...>` immediately followed by `(`), and—when a `(` follows—the
    /// call-argument list, producing a [`ExpressionNode::MethodCall`]; otherwise a plain
    /// [`ExpressionNode::MemberAccess`]. Shared by every dot/method site (postfix chain, bare
    /// identifier chain, and primitive static-call receiver).
    pub(crate) fn parse_member_access_step(
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
                p.parse_call_argument()
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
    pub(crate) fn parse_generic_static_step(
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
                p.parse_call_argument()
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
    pub(crate) fn parse_invocation_expression(&mut self) -> Result<ExpressionNode<'a>, Error> {
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
            let argument = self.parse_call_argument()?;
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
}
