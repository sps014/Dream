use super::Parser;
use crate::nodes::{ExpressionNode, StatementNode, Type};
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {
    /// Parses a block of statements enclosed in curly braces
    pub(super) fn parse_block(&mut self) -> Result<&'a [StatementNode<'a>], Error> {
        //eat the open curly brace
        self.match_token(TokenKind::CurlyOpenBracketToken);
        let mut statements = vec![];
        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            // Recover at statement granularity: a malformed statement is reported (by the failing
            // sub-parser) and skipped to the next boundary so the rest of the block still parses
            // and surfaces its own diagnostics, instead of discarding the entire enclosing block.
            match self.parse_statement() {
                Ok(statement) => statements.push(statement),
                Err(_) => self.recover_to_next_statement(),
            }
            self.ensure_progress(iter);
        }
        //eat the close curly brace
        self.match_token(TokenKind::CurlyCloseBracketToken);
        Ok(self.arena.alloc_slice_fill_iter(statements))
    }
    /// Parses a single statement based on the current token
    /// Maps a compound-assignment token (`+=`, `-=`, ...) to the plain binary operator it expands
    /// to. Returns `None` for any other token kind.
    pub(super) fn compound_assign_operator(kind: TokenKind) -> Option<TokenKind> {
        match kind {
            TokenKind::PlusEqualToken => Some(TokenKind::PlusToken),
            TokenKind::MinusEqualToken => Some(TokenKind::MinusToken),
            TokenKind::StarEqualToken => Some(TokenKind::StarToken),
            TokenKind::SlashEqualToken => Some(TokenKind::SlashToken),
            TokenKind::ModulusEqualToken => Some(TokenKind::ModulusToken),
            _ => None,
        }
    }

    /// Computes the integer code point of a char literal token (text still includes the
    /// surrounding single quotes), resolving common escape sequences.
    pub(super) fn char_literal_value(text: &str) -> i32 {
        let inner = text.trim_matches('\'');
        let mut chars = inner.chars();
        match chars.next() {
            Some('\\') => match chars.next() {
                Some('n') => '\n' as i32,
                Some('t') => '\t' as i32,
                Some('r') => '\r' as i32,
                Some('0') => 0,
                Some('\\') => '\\' as i32,
                Some('\'') => '\'' as i32,
                Some('"') => '"' as i32,
                Some(other) => other as i32,
                None => 0,
            },
            Some(c) => c as i32,
            None => 0,
        }
    }

    /// The source text for a plain binary operator token, used when synthesizing nodes for
    /// desugared compound assignments and increments.
    pub(super) fn operator_text(kind: TokenKind) -> String {
        match kind {
            TokenKind::PlusToken => "+",
            TokenKind::MinusToken => "-",
            TokenKind::StarToken => "*",
            TokenKind::SlashToken => "/",
            TokenKind::ModulusToken => "%",
            _ => "",
        }
        .to_string()
    }

    /// Builds the appropriate assignment statement for a parsed lvalue expression and value.
    pub(super) fn make_assignment_statement(
        &mut self,
        target: ExpressionNode<'a>,
        value: ExpressionNode<'a>,
        cur: &SyntaxToken,
    ) -> Result<StatementNode<'a>, Error> {
        match target {
            ExpressionNode::Identifier(id) => Ok(StatementNode::Assignment(id, value)),
            ExpressionNode::IndexAccess(arr, idx) => {
                Ok(StatementNode::IndexAssignment(arr, idx, value))
            }
            ExpressionNode::MemberAccess(obj, member) => {
                Ok(StatementNode::MemberAssignment(obj, member, value))
            }
            _ => {
                self.diagnostics
                    .report_error("Invalid assignment target".to_string(), Some(cur.position));
                Ok(StatementNode::Break(None))
            }
        }
    }

    pub(super) fn parse_statement(&mut self) -> Result<StatementNode<'a>, Error> {
        let cur = self.current_token();
        match cur.kind {
            TokenKind::LetToken | TokenKind::ConstToken => Ok(self.parse_declaration()?),
            TokenKind::ReturnToken => Ok(self.parse_return()?),
            TokenKind::IfToken => Ok(self.parse_if_else()?),
            TokenKind::WhileToken => Ok(self.parse_while()?),
            TokenKind::DoToken => Ok(self.parse_do_while()?),
            TokenKind::ForToken => Ok(self.parse_for()?),
            TokenKind::SwitchToken => Ok(self.parse_switch()?),
            TokenKind::BreakToken => Ok(self.parse_break()?),
            TokenKind::ContinueToken => Ok(self.parse_continue()?),
            // `await <future-expr>;` as a statement, discarding the resolved value.
            TokenKind::AwaitToken => {
                let expr = self.parse_expression(0)?;
                self.match_token(TokenKind::SemicolonToken);
                match expr {
                    ExpressionNode::Await(inner) => Ok(StatementNode::AwaitStmt(inner.clone())),
                    other => Ok(StatementNode::AwaitStmt(other)),
                }
            }
            // A loop label: `name: while (...) { ... }` (also `for`/`do`).
            TokenKind::IdentifierToken if self.peek_token(1).kind == TokenKind::ColonToken => {
                let label = self.match_token(TokenKind::IdentifierToken);
                self.match_token(TokenKind::ColonToken);
                let inner = self.parse_statement()?;
                let inner_ref = self.arena.alloc(inner);
                Ok(StatementNode::Labeled(label.text, inner_ref))
            }
            TokenKind::IdentifierToken => {
                // Parse an expression first
                let expr = self.parse_primary_expression()?;

                if self.current_token().kind == TokenKind::EqualToken {
                    self.match_token(TokenKind::EqualToken);
                    let value = self.parse_expression(0)?;
                    self.match_token(TokenKind::SemicolonToken);
                    self.make_assignment_statement(expr, value, &cur)
                } else if let Some(plain_kind) =
                    Self::compound_assign_operator(self.current_token().kind)
                {
                    // Compound assignment `target OP= rhs` desugars to `target = target OP (rhs)`.
                    let op_tok = self.next_token();
                    let rhs = self.parse_expression(0)?;
                    self.match_token(TokenKind::SemicolonToken);
                    let plain_token = SyntaxToken::new(
                        plain_kind,
                        op_tok.position,
                        Self::operator_text(plain_kind),
                    );
                    let left_operand = self.arena.alloc(expr.clone());
                    let value =
                        ExpressionNode::Binary(left_operand, plain_token, self.arena.alloc(rhs));
                    self.make_assignment_statement(expr, value, &cur)
                } else if matches!(
                    self.current_token().kind,
                    TokenKind::PlusPlusToken | TokenKind::MinusMinusToken
                ) {
                    // `target++` / `target--` desugars to `target = target +/- 1`.
                    let op_tok = self.next_token();
                    let plain_kind = if op_tok.kind == TokenKind::PlusPlusToken {
                        TokenKind::PlusToken
                    } else {
                        TokenKind::MinusToken
                    };
                    self.match_token(TokenKind::SemicolonToken);
                    let plain_token = SyntaxToken::new(
                        plain_kind,
                        op_tok.position,
                        Self::operator_text(plain_kind),
                    );
                    let one_token =
                        SyntaxToken::new(TokenKind::NumberToken, op_tok.position, "1".to_string());
                    let one = ExpressionNode::Literal(Type::Integer(one_token));
                    let left_operand = self.arena.alloc(expr.clone());
                    let value =
                        ExpressionNode::Binary(left_operand, plain_token, self.arena.alloc(one));
                    self.make_assignment_statement(expr, value, &cur)
                } else if self.current_token().kind == TokenKind::SemicolonToken {
                    self.match_token(TokenKind::SemicolonToken);
                    match expr {
                        ExpressionNode::FunctionCall(name, generic_args, params) => Ok(
                            StatementNode::FunctionInvocation(name, generic_args, params),
                        ),
                        ExpressionNode::MethodCall(obj, member, generic_args, params) => Ok(
                            StatementNode::MethodInvocation(obj, member, generic_args, params),
                        ),
                        _ => {
                            self.diagnostics.report_error(
                                "Expected function call but found expression".to_string(),
                                Some(cur.position),
                            );
                            Ok(StatementNode::ExpressionStatement(expr))
                        }
                    }
                } else {
                    self.diagnostics.report_error(
                        format!(
                            "Unexpected token {:?} after expression",
                            self.current_token().kind
                        ),
                        Some(self.current_token().position),
                    );
                    self.recover_to_next_statement();
                    Ok(StatementNode::ExpressionStatement(expr))
                }
            }
            _ => {
                self.diagnostics.report_error(
                    format!(
                        "Expected statement but found {:?} at {}",
                        cur.text,
                        cur.position.get_point_str()
                    ),
                    Some(cur.position),
                );
                self.recover_to_next_statement();
                Ok(StatementNode::Break(None)) // dummy
            }
        }
    }

    /// Recovers from an error by consuming tokens until a recognizable statement boundary.
    fn recover_to_next_statement(&mut self) {
        // If the current token is already a strong synchronization point, do not skip it.
        // This prevents swallowing `}` or `class` when they unexpectedly terminate a statement.
        match self.current_token().kind {
            TokenKind::ClassToken
            | TokenKind::StructToken
            | TokenKind::FunToken
            | TokenKind::LetToken
            | TokenKind::ConstToken
            | TokenKind::ForToken
            | TokenKind::IfToken
            | TokenKind::WhileToken
            | TokenKind::DoToken
            | TokenKind::ReturnToken
            | TokenKind::SwitchToken
            | TokenKind::CurlyCloseBracketToken
            | TokenKind::EndOfFileToken => {
                return;
            }
            _ => {}
        }

        self.next_token(); // skip the erroneous token

        while self.current_token().kind != TokenKind::EndOfFileToken {
            let kind = self.current_token().kind;
            if kind == TokenKind::SemicolonToken {
                self.next_token(); // consume semicolon
                return;
            }

            match kind {
                TokenKind::ClassToken
                | TokenKind::StructToken
                | TokenKind::FunToken
                | TokenKind::LetToken
                | TokenKind::ConstToken
                | TokenKind::ForToken
                | TokenKind::IfToken
                | TokenKind::WhileToken
                | TokenKind::DoToken
                | TokenKind::ReturnToken
                | TokenKind::SwitchToken
                | TokenKind::CurlyCloseBracketToken => {
                    return;
                }
                _ => {
                    self.next_token();
                }
            }
        }
    }

    /// Parses a variable declaration (e.g., `let x = 5;` or `let x: int[] = [1];`)
    pub(super) fn parse_declaration(&mut self) -> Result<StatementNode<'a>, Error> {
        // Consume `let` or `const`; `const` marks the binding immutable.
        let is_const = self.current_token().kind == TokenKind::ConstToken;
        if is_const {
            self.match_token(TokenKind::ConstToken);
        } else {
            self.match_token(TokenKind::LetToken);
        }
        let identifier = self.match_token(TokenKind::IdentifierToken);

        // Optional type annotation
        let mut type_annotation = None;
        if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            type_annotation = Some(self.parse_type()?);
        }

        //eat the equal sign
        self.match_token(TokenKind::EqualToken);
        let expression = self.parse_expression(0)?;
        //eat the semicolon
        self.match_token(TokenKind::SemicolonToken);
        Ok(StatementNode::Declaration(
            identifier,
            type_annotation,
            expression,
            is_const,
        ))
    }
    /// Parses a return statement
    pub(super) fn parse_return(&mut self) -> Result<StatementNode<'a>, Error> {
        //eat the return keyword
        self.match_token(TokenKind::ReturnToken);
        let mut expression: Option<ExpressionNode> = None;
        if self.current_token().kind != TokenKind::SemicolonToken {
            expression = Some(self.parse_expression(0)?);
        }

        //eat the semicolon
        self.match_token(TokenKind::SemicolonToken);
        Ok(StatementNode::Return(expression))
    }
    /// Parses an if-else statement, including else-if chains
    pub(super) fn parse_if_else(&mut self) -> Result<StatementNode<'a>, Error> {
        //eat the if keyword
        self.match_token(TokenKind::IfToken);
        self.match_token(TokenKind::OpenParenthesisToken);
        let condition = self.parse_expression(0)?;
        self.match_token(TokenKind::CloseParenthesisToken);
        let then_branch = self.parse_block()?;
        let mut else_ifs = vec![];
        while self.current_token().kind == TokenKind::ElseToken {
            //eat the else keyword
            self.match_token(TokenKind::ElseToken);
            if self.current_token().kind == TokenKind::IfToken {
                //eat the if keyword
                self.match_token(TokenKind::IfToken);
                self.match_token(TokenKind::OpenParenthesisToken);
                let condition = self.parse_expression(0)?;
                self.match_token(TokenKind::CloseParenthesisToken);
                let then_branch = self.parse_block()?;
                else_ifs.push((condition, then_branch));
            } else {
                let else_branch = self.parse_block()?;
                return Ok(StatementNode::IfElse(
                    condition,
                    then_branch,
                    else_ifs,
                    Some(else_branch),
                ));
            }
        }

        Ok(StatementNode::IfElse(
            condition,
            then_branch,
            else_ifs,
            None,
        ))
    }

    /// Parses a for loop statement
    pub(super) fn parse_for(&mut self) -> Result<StatementNode<'a>, Error> {
        self.match_token(TokenKind::ForToken);
        self.match_token(TokenKind::OpenParenthesisToken);

        // For-each form: `for (let <var> in <iterable>) { ... }`.
        if self.current_token().kind == TokenKind::LetToken
            && self.peek_token(1).kind == TokenKind::IdentifierToken
            && self.peek_token(2).kind == TokenKind::InToken
        {
            self.match_token(TokenKind::LetToken);
            let element = self.match_token(TokenKind::IdentifierToken);
            self.match_token(TokenKind::InToken);
            let iterable = self.parse_expression(0)?;
            self.match_token(TokenKind::CloseParenthesisToken);
            let body = self.parse_block()?;

            let n = self.foreach_counter;
            self.foreach_counter += 1;
            let index_name = crate::nodes::types::foreach_index_local(n);
            let array_name = crate::nodes::types::foreach_array_local(n);
            return Ok(StatementNode::ForEach(
                element, iterable, index_name, array_name, body,
            ));
        }

        let mut init: Option<&'a StatementNode<'a>> = None;
        if self.current_token().kind != TokenKind::SemicolonToken {
            if self.current_token().kind == TokenKind::LetToken {
                init = Some(self.arena.alloc(self.parse_declaration()?));
            } else {
                init = Some(self.arena.alloc(self.parse_statement()?));
            }
        } else {
            self.match_token(TokenKind::SemicolonToken);
        }

        let mut condition = None;
        if self.current_token().kind != TokenKind::SemicolonToken {
            condition = Some(self.parse_expression(0)?);
        }
        self.match_token(TokenKind::SemicolonToken);

        let mut increment: Option<&'a StatementNode<'a>> = None;
        if self.current_token().kind != TokenKind::CloseParenthesisToken {
            // Parse the increment assignment expression (without semicolon)
            let expr = self.parse_primary_expression()?;
            self.match_token(TokenKind::EqualToken);
            let value = self.parse_expression(0)?;

            let stmt = match expr {
                ExpressionNode::Identifier(id) => StatementNode::Assignment(id, value),
                ExpressionNode::IndexAccess(arr, idx) => {
                    StatementNode::IndexAssignment(arr, idx, value)
                }
                ExpressionNode::MemberAccess(obj, member) => {
                    StatementNode::MemberAssignment(obj, member, value)
                }
                _ => {
                    self.diagnostics.report_error(
                        "Invalid assignment target in for loop increment".to_string(),
                        Some(self.current_token().position),
                    );
                    StatementNode::Break(None)
                }
            };
            increment = Some(self.arena.alloc(stmt));
        }
        self.match_token(TokenKind::CloseParenthesisToken);

        let body = self.parse_block()?;
        Ok(StatementNode::For(init, condition, increment, body))
    }

    /// Parses a while loop statement
    pub(super) fn parse_while(&mut self) -> Result<StatementNode<'a>, Error> {
        //eat the while keyword
        self.match_token(TokenKind::WhileToken);
        self.match_token(TokenKind::OpenParenthesisToken);
        let condition = self.parse_expression(0)?;
        self.match_token(TokenKind::CloseParenthesisToken);
        let body = self.parse_block()?;
        Ok(StatementNode::While(condition, body))
    }
    /// Parses a do-while loop: `do { body } while (condition);`.
    pub(super) fn parse_do_while(&mut self) -> Result<StatementNode<'a>, Error> {
        self.match_token(TokenKind::DoToken);
        let body = self.parse_block()?;
        self.match_token(TokenKind::WhileToken);
        self.match_token(TokenKind::OpenParenthesisToken);
        let condition = self.parse_expression(0)?;
        self.match_token(TokenKind::CloseParenthesisToken);
        self.match_token(TokenKind::SemicolonToken);
        Ok(StatementNode::DoWhile(body, condition))
    }
    /// Parses a `switch` statement. After the shared `switch (subject) {` header, the body decides
    /// the form: a leading `case`/`default` (or an empty body) is the C-style form
    /// `switch (expr) { case v1, v2: stmt* case v3: stmt* default: stmt* }` (each case body runs
    /// until the next `case`/`default`/`}` with no implicit fallthrough); anything else is the
    /// pattern-matching form `switch (expr) { pattern [if guard] => body, ... }`, which is parsed as
    /// an [`ExpressionNode::Switch`] and wrapped in an `ExpressionStatement` (no trailing `;`).
    pub(super) fn parse_switch(&mut self) -> Result<StatementNode<'a>, Error> {
        let subject = self.parse_switch_header()?;

        // Pattern form: the body starts with a pattern/`=>` arm rather than `case`/`default`.
        if !matches!(
            self.current_token().kind,
            TokenKind::CaseToken | TokenKind::DefaultToken | TokenKind::CurlyCloseBracketToken
        ) {
            let arms = self.parse_switch_arms()?;
            let expr = ExpressionNode::Switch(self.arena.alloc(subject), arms);
            return Ok(StatementNode::ExpressionStatement(expr));
        }

        let mut cases: Vec<(Vec<ExpressionNode<'a>>, &'a [StatementNode<'a>])> = Vec::new();
        let mut default_body: Option<&'a [StatementNode<'a>]> = None;

        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            if self.current_token().kind == TokenKind::CaseToken {
                self.match_token(TokenKind::CaseToken);
                // One or more comma-separated label expressions.
                let mut labels = vec![self.parse_expression(0)?];
                while self.current_token().kind == TokenKind::CommaToken {
                    self.match_token(TokenKind::CommaToken);
                    labels.push(self.parse_expression(0)?);
                }
                self.match_token(TokenKind::ColonToken);
                let body = self.parse_case_body()?;
                cases.push((labels, body));
            } else if self.current_token().kind == TokenKind::DefaultToken {
                self.match_token(TokenKind::DefaultToken);
                self.match_token(TokenKind::ColonToken);
                let body = self.parse_case_body()?;
                if default_body.is_some() {
                    self.diagnostics.report_error(
                        "Multiple 'default' clauses in switch statement".to_string(),
                        Some(self.current_token().position),
                    );
                }
                default_body = Some(body);
            } else {
                self.diagnostics.report_error(
                    format!(
                        "Expected 'case' or 'default' in switch body but found {:?}",
                        self.current_token().text
                    ),
                    Some(self.current_token().position),
                );
                self.next_token();
            }
            self.ensure_progress(iter);
        }

        self.match_token(TokenKind::CurlyCloseBracketToken);
        Ok(StatementNode::Switch(subject, cases, default_body))
    }

    /// Parses the statements of a single `case`/`default` clause, up to (but not consuming) the
    /// next `case`, `default`, or the closing `}`.
    pub(super) fn parse_case_body(&mut self) -> Result<&'a [StatementNode<'a>], Error> {
        let mut statements = vec![];
        while self.current_token().kind != TokenKind::CaseToken
            && self.current_token().kind != TokenKind::DefaultToken
            && self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            statements.push(self.parse_statement()?);
            self.ensure_progress(iter);
        }
        Ok(self.arena.alloc_slice_fill_iter(statements))
    }

    /// Parses a break statement, with an optional target label: `break;` or `break outer;`.
    pub(super) fn parse_break(&mut self) -> Result<StatementNode<'a>, Error> {
        self.match_token(TokenKind::BreakToken);
        let label = if self.current_token().kind == TokenKind::IdentifierToken {
            Some(self.match_token(TokenKind::IdentifierToken).text)
        } else {
            None
        };
        self.match_token(TokenKind::SemicolonToken);
        Ok(StatementNode::Break(label))
    }
    /// Parses a continue statement, with an optional target label: `continue;` or `continue outer;`.
    pub(super) fn parse_continue(&mut self) -> Result<StatementNode<'a>, Error> {
        self.match_token(TokenKind::ContinueToken);
        let label = if self.current_token().kind == TokenKind::IdentifierToken {
            Some(self.match_token(TokenKind::IdentifierToken).text)
        } else {
            None
        };
        self.match_token(TokenKind::SemicolonToken);
        Ok(StatementNode::Continue(label))
    }
}
