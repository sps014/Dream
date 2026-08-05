use super::super::Parser;
use crate::nodes::Visibility;
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {

    /// Parses a struct declaration
    pub(crate) fn parse_struct_declaration(
        &mut self,
    ) -> Result<crate::nodes::struct_node::StructDeclarationNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();

        let attributes = self.parse_attributes();

        // `public`/`internal` and `sealed` are modifiers that may appear in either order before the
        // `class`/`struct` keyword.
        let mut visibility = Visibility::Private;
        let mut is_sealed = false;
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

        // `ref struct` is a stack-only value type: in addition to being a value type (inline, copy
        // semantics), the compiler rejects any use of it that would let it escape the current stack
        // frame (stored in a heap object, used as a generic type argument, captured by a closure, or
        // crossing an `async` boundary — see `Analyzer::check_ref_struct_escapes`).
        let is_ref_struct = self.current_token().kind == TokenKind::RefToken;
        if is_ref_struct {
            self.match_token(TokenKind::RefToken);
        }

        // A value type is introduced with `struct`; a reference type with `class`. Both share the
        // same declaration shape and AST node, differing only in the `is_value` flag.
        let is_value = is_ref_struct || self.current_token().kind == TokenKind::StructToken;
        if self.current_token().kind == TokenKind::ClassToken {
            if is_ref_struct {
                self.diagnostics.report_error(
                    "'ref' may only precede 'struct', not 'class'".to_string(),
                    Some(self.current_token().position),
                );
            }
            self.match_token(TokenKind::ClassToken);
        } else {
            self.match_token(TokenKind::StructToken);
        }
        let mut struct_name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut struct_name, first_trivia);

        let (generic_parameters, generic_constraints) = self.take_generic_params();

        // Optional `: Iface1, Container<int>, ...` implements clause. Each entry is a (possibly
        // generic) interface type the class declares it satisfies; the class must provide a matching
        // method for every interface method (validated during semantic analysis).
        let mut implements = Vec::new();
        if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            loop {
                let iter = self.current_token_index;
                implements.push(self.parse_type()?);
                if self.current_token().kind == TokenKind::CommaToken {
                    self.match_token(TokenKind::CommaToken);
                } else {
                    break;
                }
                self.ensure_progress(iter);
            }
        }

        self.match_token(TokenKind::CurlyOpenBracketToken);

        let mut fields = Vec::new();
        let mut methods = Vec::new();
        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            let field_attributes = self.parse_attributes();

            // Classify the member by looking past any leading `public`/`static`/`async`: a
            // method (`fun`, `static fun`, `constructor`/`del`, `extern fun`) is dispatched to
            // `parse_function` (which consumes its own modifiers), otherwise it is a field.
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
            let is_ctor_dtor = core.kind == TokenKind::IdentifierToken
                && crate::nodes::types::is_special_member_name(&core.text)
                && self.peek_token(m + 1).kind == TokenKind::OpenParenthesisToken;

            // TypeScript-style property accessor: `get name(...)` / `set name(...)`. `get`/`set`
            // are contextual keywords (still ordinary identifiers/field names elsewhere), so this
            // only binds when the next token is a property name followed by a parameter list.
            let is_accessor = core.kind == TokenKind::IdentifierToken
                && crate::nodes::function::AccessorKind::from_keyword(&core.text).is_some()
                && self.peek_token(m + 1).kind == TokenKind::IdentifierToken
                && self.peek_token(m + 2).kind == TokenKind::OpenParenthesisToken;
            if core.kind == TokenKind::FunToken
                || core.kind == TokenKind::ExternToken
                || is_ctor_dtor
                || is_accessor
            {
                methods.push(self.parse_function(Some(field_attributes))?);
            } else {
                // Fields are private by default; an explicit `public`/`internal` exposes them.
                // `weak`/`unowned` are storage qualifiers that break ARC reference cycles (see
                // `docs/language/memory.md`); any order/combination with the visibility modifier is
                // accepted here, with `weak`+`unowned` together rejected during semantic analysis.
                let mut field_visibility = Visibility::Private;
                let mut field_weak = false;
                let mut field_unowned = false;
                loop {
                    if self.try_consume_visibility(&mut field_visibility) {
                        continue;
                    }
                    match self.current_token().kind {
                        TokenKind::WeakToken => {
                            self.match_token(TokenKind::WeakToken);
                            field_weak = true;
                        }
                        TokenKind::UnownedToken => {
                            self.match_token(TokenKind::UnownedToken);
                            field_unowned = true;
                        }
                        _ => break,
                    }
                }
                let field_name = self.match_token(TokenKind::IdentifierToken);
                self.match_token(TokenKind::ColonToken);

                // Parse the full type (supporting generic args like `Map<string, JsonValue>`
                // and arrays) and store its canonical spelling on the field.
                let type_position = self.current_token().position;
                let parsed_type = self.parse_type()?;
                let field_type_token = crate::token::syntax_token::SyntaxToken::new(
                    TokenKind::IdentifierToken,
                    type_position,
                    parsed_type.get_type(),
                );

                self.match_token(TokenKind::SemicolonToken);
                fields.push(crate::nodes::struct_node::StructFieldNode {
                    attributes: field_attributes,
                    name: field_name,
                    visibility: field_visibility,
                    is_weak: field_weak,
                    is_unowned: field_unowned,
                    type_token: field_type_token,
                    field_type: parsed_type,
                });
            }
            self.ensure_progress(iter);
        }

        self.match_token(TokenKind::CurlyCloseBracketToken);
        let mut decl = crate::nodes::struct_node::StructDeclarationNode::new(
            attributes,
            struct_name,
            generic_parameters,
            fields,
            methods,
            visibility,
        );
        decl.implements = implements;
        decl.is_value = is_value;
        decl.is_ref_struct = is_ref_struct;
        decl.is_sealed = is_sealed;
        decl.generic_constraints = generic_constraints;
        Ok(decl)
    }
}
