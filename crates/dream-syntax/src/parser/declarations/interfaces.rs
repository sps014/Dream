use super::super::Parser;
use super::functions::FunctionModifiers;
use crate::nodes::{
    FunctionNode, StatementNode, Type,
    Visibility,
};
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {

    /// Parses an `interface` declaration:
    /// `[public] interface Name [<T>] [: Parent (+ Parent)*] { method* }`.
    pub(crate) fn parse_interface_declaration(
        &mut self,
    ) -> Result<crate::nodes::InterfaceDeclarationNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();
        let attributes = self.parse_attributes();
        let doc_trivia = Self::recover_doc_trivia(first_trivia, &attributes);

        let mut visibility = Visibility::Private;
        self.try_consume_visibility(&mut visibility);

        self.match_token(TokenKind::InterfaceToken);
        let mut name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut name, doc_trivia);

        let (generic_parameters, generic_constraints) = self.take_generic_params();

        // Optional parent list: `: Collection<T> + Serializable`. Uses `+` (same as generic
        // bounds); class `implements` lists stay comma-separated.
        let mut parents = Vec::new();
        if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            loop {
                let iter = self.current_token_index;
                match self.parse_type() {
                    Ok(t) => parents.push(t),
                    Err(_) => break,
                }
                if self.current_token().kind != TokenKind::PlusToken {
                    break;
                }
                self.match_token(TokenKind::PlusToken);
                self.ensure_progress(iter);
            }
        }

        self.match_token(TokenKind::CurlyOpenBracketToken);

        let mut methods = Vec::new();
        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            let method_attributes = self.parse_attributes();
            methods.push(self.parse_interface_method(method_attributes)?);
            self.ensure_progress(iter);
        }

        self.match_token(TokenKind::CurlyCloseBracketToken);
        let mut decl = crate::nodes::InterfaceDeclarationNode::new(
            attributes,
            name,
            generic_parameters,
            methods,
            visibility,
        );
        decl.generic_constraints = generic_constraints;
        decl.parents = parents;
        Ok(decl)
    }

    /// Parses one interface method: `[public] [static] fun Name[<T>](params)[: ret] ;` for a
    /// signature-only method, or `... { ... }` for a *default* method whose body implementing
    /// classes inherit when they omit the method.
    pub(crate) fn parse_interface_method(
        &mut self,
        attributes: Vec<crate::nodes::AttributeNode>,
    ) -> Result<FunctionNode<'a>, Error> {
        let FunctionModifiers {
            is_async,
            visibility,
            is_static,
            is_extern: _,
        } = self.parse_function_modifiers();

        self.match_token(TokenKind::FunToken);
        let function_name = self.match_token(TokenKind::IdentifierToken);
        let (generic_parameters, generic_constraints) = self.take_generic_params();
        let params = self.parse_formal_parameters()?;
        let mut return_type: Option<Type> = None;
        if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            return_type = Some(self.parse_type()?);
        }

        // A `{ ... }` body makes this a *default* method: implementing classes that omit it inherit
        // this body. A signature-only method ends with `;`.
        let (body, is_default): (&'a [StatementNode<'a>], bool) =
            if self.current_token().kind == TokenKind::CurlyOpenBracketToken {
                (self.parse_block()?, true)
            } else {
                self.match_token(TokenKind::SemicolonToken);
                (self.arena.alloc_slice_fill_iter(std::iter::empty()), false)
            };

        let mut node = FunctionNode::new(
            attributes,
            function_name,
            generic_parameters,
            return_type,
            params,
            body,
            visibility,
        );
        node.is_static = is_static;
        node.is_async = is_async;
        node.is_default_impl = is_default;
        node.generic_constraints = generic_constraints;
        Ok(node)
    }

    /// Parses an `extend Type { ... }` block: a set of methods attached to an existing type
    /// (a primitive, `object`, array `T[]`, or a struct). The body holds method declarations only
    /// (no fields, no `constructor`/`del`).
    pub(crate) fn parse_extend_declaration(
        &mut self,
    ) -> Result<crate::nodes::ExtendNode<'a>, Error> {
        self.match_token(TokenKind::ExtendToken);

        let mut target = if self.current_token().kind == TokenKind::DataTypeToken {
            self.match_token(TokenKind::DataTypeToken)
        } else {
            self.match_token(TokenKind::IdentifierToken)
        };
        // Optional `[]` suffix so `extend int[] : IndexedCollection<int>` is a valid target.
        // Nested arrays (`int[][]`) are allowed the same way as in `parse_type`.
        while self.current_token().kind == TokenKind::OpenBracketToken {
            self.match_token(TokenKind::OpenBracketToken);
            self.match_token(TokenKind::CloseBracketToken);
            target.text.push_str("[]");
        }

        let (generic_parameters, generic_constraints) = self.take_generic_params();

        // Optional `: Iface1, Comparable<int>, ...` implements clause: an `extend` block may declare
        // that its target satisfies one or more interfaces (e.g. `extend int : Comparable<int>`),
        // making primitives and other non-class types participate in interface dispatch. The block
        // must provide a matching method for every interface method (validated in analysis).
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

        let mut methods = Vec::new();
        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            let field_attributes = self.parse_attributes();
            if self.current_token().kind == TokenKind::FunToken
                || self.current_token().kind == TokenKind::PublicToken
                || self.current_token().kind == TokenKind::InternalToken
                || self.current_token().kind == TokenKind::StaticToken
                || self.current_token().kind == TokenKind::AsyncToken
            {
                methods.push(self.parse_function(Some(field_attributes))?);
            } else {
                let cur = self.current_token();
                self.diagnostics.report_error(
                    format!(
                        "'extend' blocks may only contain methods, but found {:?}",
                        cur.kind
                    ),
                    Some(cur.position),
                );
                self.next_token();
            }
            self.ensure_progress(iter);
        }

        self.match_token(TokenKind::CurlyCloseBracketToken);
        let mut node = crate::nodes::ExtendNode::new(target, generic_parameters, methods);
        node.generic_constraints = generic_constraints;
        node.implements = implements;
        Ok(node)
    }
}
