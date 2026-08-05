use super::super::Parser;
use crate::nodes::Visibility;
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {
    /// Parses an enum declaration. Two forms share this parser:
    /// - C-style integer enum: `enum Color { Red, Green = 5, Blue }`. Members without an explicit
    ///   value continue from the previous member's value (starting at 0).
    /// - Discriminated union: `enum Shape { Circle(radius: float), Empty }`, optionally generic
    ///   `enum Option<T> { Some(value: T), None }`. A variant carries a parenthesized payload of
    ///   `name: Type` fields; the variant's `value` is its discriminant (sequential from 0).
    ///
    /// Methods may be declared in the body alongside variants (`public fun is_some(): bool { ... }`),
    /// using the same member classification as classes (look past `public`/`static`/`async` for `fun`).
    pub(crate) fn parse_enum_declaration(
        &mut self,
    ) -> Result<crate::nodes::EnumDeclarationNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();
        let attributes = self.parse_attributes();

        // A doc comment that preceded the first attribute (e.g. above `@json`) is consumed with the
        // attribute. Recover it so the comment still reaches the enum name token for hover/LSP.
        let doc_trivia = Self::recover_doc_trivia(first_trivia, &attributes);

        // Optional `public`/`internal` / `sealed` modifiers (any order) before the `enum` keyword.
        let mut is_sealed = false;
        let mut visibility = Visibility::Private;
        loop {
            if self.try_consume_visibility(&mut visibility) {
                continue;
            }
            match self.current_token().kind {
                TokenKind::SealedToken => {
                    self.match_token(TokenKind::SealedToken);
                    is_sealed = true;
                }
                _ => break,
            }
        }

        self.match_token(TokenKind::EnumToken);
        let mut name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut name, doc_trivia);

        let (generic_parameters, generic_constraints) = self.take_generic_params();

        self.match_token(TokenKind::CurlyOpenBracketToken);

        let mut variants = Vec::new();
        let mut methods = Vec::new();
        let mut next_value: i32 = 0;
        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let index_before = self.current_token_index;
            let member_attributes = self.parse_attributes();

            // Classify like class members: look past visibility/static/async for `fun` / accessors.
            let mut m = 0;
            while matches!(
                self.peek_token(m).kind,
                TokenKind::PublicToken
                    | TokenKind::InternalToken
                    | TokenKind::StaticToken
                    | TokenKind::AsyncToken
            ) {
                m += 1;
            }
            let core = self.peek_token(m);
            let is_accessor = core.kind == TokenKind::IdentifierToken
                && crate::nodes::function::AccessorKind::from_keyword(&core.text).is_some()
                && self.peek_token(m + 1).kind == TokenKind::IdentifierToken
                && self.peek_token(m + 2).kind == TokenKind::OpenParenthesisToken;
            let is_special = core.kind == TokenKind::IdentifierToken
                && crate::nodes::types::is_special_member_name(&core.text)
                && self.peek_token(m + 1).kind == TokenKind::OpenParenthesisToken;

            if core.kind == TokenKind::FunToken
                || core.kind == TokenKind::ExternToken
                || is_accessor
                || is_special
            {
                if is_special {
                    self.diagnostics.report_error(
                        "Enums cannot declare 'constructor' or 'del'".to_string(),
                        Some(core.position),
                    );
                }
                methods.push(self.parse_function(Some(member_attributes))?);
            } else {
                let variant_name = self.match_token(TokenKind::IdentifierToken);

                // A payload `(name: Type, ...)` makes this a discriminated-union variant.
                let mut fields = Vec::new();
                if self.current_token().kind == TokenKind::OpenParenthesisToken {
                    self.match_token(TokenKind::OpenParenthesisToken);
                    fields = self.parse_delimited_list(TokenKind::CloseParenthesisToken, |p| {
                        p.parse_variant_field()
                    })?;
                }

                // C-style explicit value (`Green = 5`); only meaningful for payload-less variants.
                let value = if self.current_token().kind == TokenKind::EqualToken {
                    self.match_token(TokenKind::EqualToken);
                    let num = self.match_token(TokenKind::NumberToken);
                    num.text.parse::<i32>().unwrap_or(next_value)
                } else {
                    next_value
                };
                next_value = value + 1;
                variants.push(crate::nodes::EnumVariantNode {
                    name: variant_name,
                    fields,
                    value,
                });

                if self.current_token().kind == TokenKind::CommaToken {
                    self.match_token(TokenKind::CommaToken);
                }
            }
            // Safety: never spin on an unexpected token.
            if self.current_token_index == index_before {
                self.next_token();
            }
        }
        self.match_token(TokenKind::CurlyCloseBracketToken);
        let mut decl =
            crate::nodes::EnumDeclarationNode::new(attributes, name, generic_parameters, variants);
        decl.generic_constraints = generic_constraints;
        decl.methods = methods;
        decl.is_sealed = is_sealed;
        decl.visibility = visibility;
        Ok(decl)
    }

    /// Parses a single discriminated-union variant payload field: `name: Type`.
    pub(crate) fn parse_variant_field(
        &mut self,
    ) -> Result<crate::nodes::struct_node::StructFieldNode, Error> {
        let field_name = self.match_token(TokenKind::IdentifierToken);
        self.match_token(TokenKind::ColonToken);
        let type_position = self.current_token().position;
        let parsed_type = self.parse_type()?;
        let field_type_token = crate::token::syntax_token::SyntaxToken::new(
            TokenKind::IdentifierToken,
            type_position,
            parsed_type.get_type(),
        );
        Ok(crate::nodes::struct_node::StructFieldNode {
            attributes: Vec::new(),
            name: field_name,
            visibility: Visibility::Public,
            is_weak: false,
            is_unowned: false,
            type_token: field_type_token,
            field_type: parsed_type,
        })
    }
}
