use super::Parser;
use crate::nodes::{AttributeNode, FunctionNode, ImportNode, ParameterNode, StatementNode, Type};
use crate::token::syntax_token::SyntaxToken;
use crate::token::syntax_trivia::SyntaxTrivia;
use crate::token::token_kind::TokenKind;
use std::io::Error;

/// The four boolean modifiers a `fun`/`constructor`/`del` declaration may carry, parsed from the
/// flexible `async`/`public`/`static`/`extern` prefix (which may appear in several orders).
#[derive(Default)]
struct FunctionModifiers {
    is_async: bool,
    is_public: bool,
    is_static: bool,
    is_extern: bool,
}

impl<'a, 'b> Parser<'a, 'b> {
    /// Recovers a doc comment that was attached to a leading attribute. When `first_trivia` (the
    /// trivia captured before attribute parsing) is empty but the first attribute carries leading
    /// trivia (the doc comment was consumed together with `@attr`), returns the attribute's trivia
    /// so it can still be threaded onto the declaration name for hover/LSP.
    fn recover_doc_trivia(
        first_trivia: Vec<SyntaxTrivia>,
        attributes: &[AttributeNode],
    ) -> Vec<SyntaxTrivia> {
        if first_trivia.is_empty() {
            if let Some(first_attr) = attributes.first() {
                if !first_attr.name.leading_trivia.is_empty() {
                    return first_attr.name.leading_trivia.clone();
                }
            }
        }
        first_trivia
    }

    /// Splices recovered doc-comment trivia onto the front of a declaration's name token so tooling
    /// sees the comment on the name even though it lexically preceded attributes/modifiers.
    fn splice_leading_trivia(name: &mut SyntaxToken, trivia: Vec<SyntaxTrivia>) {
        if !trivia.is_empty() {
            name.leading_trivia.splice(0..0, trivia);
        }
    }

    /// Parses a type alias: `type Name = ExistingType;`. The alias is recorded and resolved
    /// (erased) during `parse_type`, so it must be declared before use.
    pub(super) fn parse_type_alias(&mut self) -> Result<(), Error> {
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

    /// Parses an enum declaration. Two forms share this parser:
    /// - C-style integer enum: `enum Color { Red, Green = 5, Blue }`. Members without an explicit
    ///   value continue from the previous member's value (starting at 0).
    /// - Discriminated union: `enum Shape { Circle(radius: float), Empty }`, optionally generic
    ///   `enum Option<T> { Some(value: T), None }`. A variant carries a parenthesized payload of
    ///   `name: Type` fields; the variant's `value` is its discriminant (sequential from 0).
    pub(super) fn parse_enum_declaration(
        &mut self,
    ) -> Result<crate::nodes::EnumDeclarationNode, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();
        let attributes = self.parse_attributes();

        // A doc comment that preceded the first attribute (e.g. above `@json`) is consumed with the
        // attribute. Recover it so the comment still reaches the enum name token for hover/LSP.
        let doc_trivia = Self::recover_doc_trivia(first_trivia, &attributes);

        self.match_token(TokenKind::EnumToken);
        let mut name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut name, doc_trivia);

        // Optional generic parameters: `enum Option<T> { ... }`.
        let generic_parameters = self.parse_identifier_generic_params();

        self.match_token(TokenKind::CurlyOpenBracketToken);

        let mut variants = Vec::new();
        let mut next_value: i32 = 0;
        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let index_before = self.current_token_index;
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
            // Safety: never spin on an unexpected token.
            if self.current_token_index == index_before {
                self.next_token();
            }
        }
        self.match_token(TokenKind::CurlyCloseBracketToken);
        Ok(crate::nodes::EnumDeclarationNode::new(
            attributes,
            name,
            generic_parameters,
            variants,
        ))
    }

    /// Parses a single discriminated-union variant payload field: `name: Type`.
    fn parse_variant_field(
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
            is_public: true,
            type_token: field_type_token,
            field_type: parsed_type,
        })
    }

    /// Parses a struct declaration
    pub(super) fn parse_struct_declaration(
        &mut self,
    ) -> Result<crate::nodes::struct_node::StructDeclarationNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();

        let attributes = self.parse_attributes();

        let mut is_public = false;
        if self.current_token().kind == TokenKind::PublicToken {
            self.match_token(TokenKind::PublicToken);
            is_public = true;
        }

        // A value type is introduced with `struct`; a reference type with `class`. Both share the
        // same declaration shape and AST node, differing only in the `is_value` flag.
        let is_value = self.current_token().kind == TokenKind::StructToken;
        if is_value {
            self.match_token(TokenKind::StructToken);
        } else {
            self.match_token(TokenKind::ClassToken);
        }
        let mut struct_name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut struct_name, first_trivia);

        let generic_parameters = self.parse_identifier_generic_params();

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
                TokenKind::PublicToken | TokenKind::StaticToken | TokenKind::AsyncToken
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
                // Fields are private by default; an explicit `public` exposes them.
                let mut field_public = false;
                if self.current_token().kind == TokenKind::PublicToken {
                    self.match_token(TokenKind::PublicToken);
                    field_public = true;
                }
                let field_name = self.match_token(TokenKind::IdentifierToken);
                self.match_token(TokenKind::ColonToken);

                // Parse the full type (supporting generic args like `Map<string, JsonValue>`,
                // arrays, and nullable suffixes) and store its canonical spelling on the field.
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
                    is_public: field_public,
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
            is_public,
        );
        decl.implements = implements;
        decl.is_value = is_value;
        Ok(decl)
    }

    /// Parses an `interface` declaration: `[public] interface Name [<T>] { method-signature* }`.
    /// Interface members are body-less method signatures ending in `;` (default bodies are not
    /// supported in v1).
    pub(super) fn parse_interface_declaration(
        &mut self,
    ) -> Result<crate::nodes::InterfaceDeclarationNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();
        let attributes = self.parse_attributes();
        let doc_trivia = Self::recover_doc_trivia(first_trivia, &attributes);

        let mut is_public = false;
        if self.current_token().kind == TokenKind::PublicToken {
            self.match_token(TokenKind::PublicToken);
            is_public = true;
        }

        self.match_token(TokenKind::InterfaceToken);
        let mut name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut name, doc_trivia);

        let generic_parameters = self.parse_identifier_generic_params();

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
        Ok(crate::nodes::InterfaceDeclarationNode::new(
            attributes,
            name,
            generic_parameters,
            methods,
            is_public,
        ))
    }

    /// Parses one interface method signature: `[public] [static] fun Name[<T>](params)[: ret] ;`.
    /// The method has no body; a `{ ... }` default body is rejected (deferred to a later version)
    /// but still consumed so parsing can continue.
    fn parse_interface_method(
        &mut self,
        attributes: Vec<crate::nodes::AttributeNode>,
    ) -> Result<FunctionNode<'a>, Error> {
        let FunctionModifiers {
            is_async,
            is_public,
            is_static,
            is_extern: _,
        } = self.parse_function_modifiers();

        self.match_token(TokenKind::FunToken);
        let function_name = self.match_token(TokenKind::IdentifierToken);
        let generic_parameters = self.parse_identifier_generic_params();
        let params = self.parse_formal_parameters()?;
        let mut return_type: Option<Type> = None;
        if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            return_type = Some(self.parse_type()?);
        }

        if self.current_token().kind == TokenKind::CurlyOpenBracketToken {
            self.diagnostics.report_error(
                format!(
                    "interface method '{}' must be a signature ending with ';' (default method bodies are not supported yet)",
                    function_name.text
                ),
                Some(function_name.position),
            );
            // Consume the block so parsing can recover.
            let _ = self.parse_block()?;
        } else {
            self.match_token(TokenKind::SemicolonToken);
        }

        let empty: &'a [StatementNode<'a>] = self.arena.alloc_slice_fill_iter(std::iter::empty());
        let mut node = FunctionNode::new(
            attributes,
            function_name,
            generic_parameters,
            return_type,
            params,
            empty,
            is_public,
        );
        node.is_static = is_static;
        node.is_async = is_async;
        Ok(node)
    }

    /// Parses an `extend Type { ... }` block: a set of methods attached to an existing type
    /// (a primitive, `object`, or a struct). The body holds method declarations only (no
    /// fields, no `constructor`/`del`). The target name is normalized to its canonical primitive
    /// spelling (e.g. `String` -> `string`).
    pub(super) fn parse_extend_declaration(
        &mut self,
    ) -> Result<crate::nodes::ExtendNode<'a>, Error> {
        self.match_token(TokenKind::ExtendToken);

        let mut target = if self.current_token().kind == TokenKind::DataTypeToken {
            self.match_token(TokenKind::DataTypeToken)
        } else {
            self.match_token(TokenKind::IdentifierToken)
        };
        if let Some(canonical) = crate::nodes::types::canonical_type_name(&target.text) {
            target.text = canonical.to_string();
        }

        let generic_parameters = self.parse_identifier_generic_params();

        self.match_token(TokenKind::CurlyOpenBracketToken);

        let mut methods = Vec::new();
        while self.current_token().kind != TokenKind::CurlyCloseBracketToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            let field_attributes = self.parse_attributes();
            if self.current_token().kind == TokenKind::FunToken
                || self.current_token().kind == TokenKind::PublicToken
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
        Ok(crate::nodes::ExtendNode::new(
            target,
            generic_parameters,
            methods,
        ))
    }

    /// Parses an import statement of the form `import a.b.c;`, mapping each dotted segment to a
    /// directory separator (`a/b/c`) so resolution can append the `.dream` extension later.
    pub(super) fn parse_import(&mut self) -> Result<ImportNode, Error> {
        self.match_token(TokenKind::ImportToken);

        let first = self.match_token(TokenKind::IdentifierToken);
        let mut position = first.position;
        let mut path = first.text.clone();

        while self.current_token().kind == TokenKind::DotToken {
            self.match_token(TokenKind::DotToken);
            let segment = self.match_token(TokenKind::IdentifierToken);
            position.end = segment.position.end;
            path.push('/');
            path.push_str(&segment.text);
        }

        self.match_token(TokenKind::SemicolonToken);

        let module_name = SyntaxToken::new(TokenKind::IdentifierToken, position, path);
        Ok(ImportNode::new(module_name))
    }
    /// Parses a Type from the token stream, including array types
    pub(super) fn parse_type(&mut self) -> Result<Type, Error> {
        // Function type: `fun(param, ...): ret` (the return annotation is optional, defaulting to
        // void). Used for first-class function values and function parameters.
        if self.current_token().kind == TokenKind::FunToken {
            self.match_token(TokenKind::FunToken);
            self.match_token(TokenKind::OpenParenthesisToken);
            let params =
                self.parse_delimited_list(TokenKind::CloseParenthesisToken, |p| p.parse_type())?;
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
        // `from_token` only rejects one syntactic shape: a non-reference nullable such as `int?`.
        // Route that through the diagnostics bag (syntax's single error channel) and recover with a
        // poison type so parsing continues, rather than fabricating an `io::Error` that aborts the
        // whole parse.
        let type_position = type_token.position;
        let mut parsed_type = match Type::from_token(type_token) {
            Ok(t) => t,
            Err(e) => {
                self.diagnostics
                    .report_error(e.to_string(), Some(type_position));
                Type::Unknown
            }
        };

        // Resolve a type alias to its underlying type (unless generic args follow). Array/nullable
        // suffixes below still apply to the resolved type.
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

        // Check for nullable suffix `?`
        if self.current_token().kind == TokenKind::QuestionMarkToken {
            self.match_token(TokenKind::QuestionMarkToken);
            parsed_type = Type::Nullable(Box::new(parsed_type));
        }

        Ok(parsed_type)
    }

    pub(super) fn parse_attributes(&mut self) -> Vec<crate::nodes::AttributeNode> {
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
                    args.push(self.current_token().clone());
                    self.next_token();
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

    /// Parses the flexible function-modifier prefix (`async`/`public`/`static`/`extern`, which may
    /// appear in several orders) and reports the `public`+`extern` conflict. Consumes exactly the
    /// modifier tokens, leaving the cursor on the `fun`/constructor/`del` token.
    fn parse_function_modifiers(&mut self) -> FunctionModifiers {
        let mut m = FunctionModifiers::default();

        // `async` may appear before or after `public` (e.g. `async fun`, `public async fun`,
        // `async public fun`). Calling such a function eagerly starts a task and yields `Future<T>`.
        if self.current_token().kind == TokenKind::AsyncToken {
            self.match_token(TokenKind::AsyncToken);
            m.is_async = true;
        }

        if self.current_token().kind == TokenKind::PublicToken {
            self.match_token(TokenKind::PublicToken);
            m.is_public = true;
        }

        if self.current_token().kind == TokenKind::AsyncToken {
            self.match_token(TokenKind::AsyncToken);
            m.is_async = true;
        }

        // `static fun ...`: a method with no implicit `this`, called as `Type.method(...)`.
        if self.current_token().kind == TokenKind::StaticToken {
            self.match_token(TokenKind::StaticToken);
            m.is_static = true;
        }

        if self.current_token().kind == TokenKind::ExternToken {
            self.match_token(TokenKind::ExternToken);
            m.is_extern = true;
            if m.is_public {
                self.diagnostics.report_error(
                    "A function cannot be both 'public' and 'extern'".to_string(),
                    Some(self.current_token().position),
                );
            }
        }

        // allow `static` again in case order was reversed
        if self.current_token().kind == TokenKind::StaticToken {
            self.match_token(TokenKind::StaticToken);
            m.is_static = true;
        }

        // `static async fun ...`: allow `async` to follow `static` as well as precede it.
        if self.current_token().kind == TokenKind::AsyncToken {
            self.match_token(TokenKind::AsyncToken);
            m.is_async = true;
        }

        m
    }

    /// Parses a function declaration
    pub(super) fn parse_function(
        &mut self,
        pre_parsed_attributes: Option<Vec<crate::nodes::AttributeNode>>,
    ) -> Result<FunctionNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();

        let attributes = pre_parsed_attributes.unwrap_or_else(|| self.parse_attributes());

        // When attributes were parsed by the caller (e.g. struct members), the doc comment that
        // preceded the first attribute was consumed with it. Recover it from the attribute so the
        // comment still reaches the function name token below. (Whitespace is not trivia, so an
        // empty `first_trivia` reliably means "nothing but the attribute came before us".)
        let first_trivia = Self::recover_doc_trivia(first_trivia, &attributes);

        let FunctionModifiers {
            is_async,
            is_public,
            is_static,
            is_extern,
        } = self.parse_function_modifiers();

        // Constructor (`constructor`) / destructor (`del`) declarations omit the `fun` keyword and
        // the return type; they are lowered to ordinary methods named `constructor`/`del` and
        // dispatched specially (constructor calls, scope-exit destructor calls). They cannot be
        // marked `public`.
        if self.current_token().kind == TokenKind::IdentifierToken
            && crate::nodes::types::is_special_member_name(&self.current_token().text)
        {
            let ctor_name = self.match_token(TokenKind::IdentifierToken);
            if is_public {
                self.diagnostics.report_error(
                    format!("'{}' cannot be marked 'public'", ctor_name.text),
                    Some(ctor_name.position),
                );
            }
            let params = self.parse_formal_parameters()?;
            let block = self.parse_block()?;
            return Ok(FunctionNode::new(
                attributes, ctor_name, None, None, params, block, false,
            ));
        }

        // TypeScript-style property accessor: `get name(): T { ... }` / `set name(value: T) { ... }`.
        // Like `constructor`/`del`, these omit `fun`; `get`/`set` are contextual keywords. A getter
        // takes no parameters and declares a return type; a setter takes one parameter. The property
        // name is stored on `name`, and `accessor` records which half this is.
        let accessor_kind = if self.current_token().kind == TokenKind::IdentifierToken
            && self.peek_token(1).kind == TokenKind::IdentifierToken
        {
            crate::nodes::function::AccessorKind::from_keyword(&self.current_token().text)
        } else {
            None
        };
        if let Some(accessor_kind) = accessor_kind {
            self.match_token(TokenKind::IdentifierToken);
            let mut prop_name = self.match_token(TokenKind::IdentifierToken);
            Self::splice_leading_trivia(&mut prop_name, first_trivia);
            let params = self.parse_formal_parameters()?;
            let mut return_type: Option<Type> = None;
            if self.current_token().kind == TokenKind::ColonToken {
                self.match_token(TokenKind::ColonToken);
                return_type = Some(self.parse_type()?);
            }
            let block = self.parse_block()?;
            let mut node = FunctionNode::new(
                attributes,
                prop_name,
                None,
                return_type,
                params,
                block,
                is_public,
            );
            node.is_static = is_static;
            node.is_async = is_async;
            node.accessor = Some(accessor_kind);
            return Ok(node);
        }

        //eat the fun keyword
        self.match_token(TokenKind::FunToken);
        let mut function_name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut function_name, first_trivia);

        let generic_parameters = self.parse_identifier_generic_params();

        let params = self.parse_formal_parameters()?;
        let mut return_type: Option<Type> = None;
        if self.current_token().kind == TokenKind::ColonToken {
            //eat the colon
            self.match_token(TokenKind::ColonToken);
            return_type = Some(self.parse_type()?);
        }

        if is_extern {
            // Extern functions are lowered to WASM imports: no body, terminated by `;`.
            // An `@intrinsic` marker lets an extern function be generic. Checked inline so the
            // syntax crate stays free of any dependency on the `intrinsics` module.
            let is_intrinsic = attributes.iter().any(|a| a.name.text == "intrinsic");
            if generic_parameters.is_some() && !is_intrinsic {
                self.diagnostics.report_error(
                    "Extern functions cannot be generic unless they are marked @intrinsic"
                        .to_string(),
                    Some(function_name.position),
                );
            }
            self.match_token(TokenKind::SemicolonToken);
            let empty: &'a [StatementNode<'a>] =
                self.arena.alloc_slice_fill_iter(std::iter::empty());
            let mut node = FunctionNode::new(
                attributes,
                function_name.clone(),
                generic_parameters,
                return_type,
                params,
                empty,
                false,
            );
            node.is_extern = true;
            node.is_static = is_static;
            node.is_async = is_async;
            return Ok(node);
        }

        let block = self.parse_block()?;
        let mut node = FunctionNode::new(
            attributes,
            function_name,
            generic_parameters,
            return_type,
            params,
            block,
            is_public,
        );
        node.is_static = is_static;
        node.is_async = is_async;
        Ok(node)
    }

    /// Parses a top-level variable declaration: an optional `public`/`static` modifier pair,
    /// then `let`/`const`, a name, an optional `: type` annotation, a required initializer, and a
    /// terminating `;`. Returns the assembled [`GlobalVariableNode`].
    pub(super) fn parse_global_variable(
        &mut self,
    ) -> Result<crate::nodes::GlobalVariableNode<'a>, Error> {
        let first_trivia = self.current_token().leading_trivia.clone();

        // `public` and `static` may appear in either order before `let`/`const`.
        let mut is_public = false;
        let mut is_static = false;
        loop {
            match self.current_token().kind {
                TokenKind::PublicToken => {
                    self.match_token(TokenKind::PublicToken);
                    is_public = true;
                }
                TokenKind::StaticToken => {
                    self.match_token(TokenKind::StaticToken);
                    is_static = true;
                }
                _ => break,
            }
        }

        let is_const = self.current_token().kind == TokenKind::ConstToken;
        if is_const {
            self.match_token(TokenKind::ConstToken);
        } else {
            self.match_token(TokenKind::LetToken);
        }

        let mut name = self.match_token(TokenKind::IdentifierToken);
        Self::splice_leading_trivia(&mut name, first_trivia);

        let declared_type = if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            Some(self.parse_type()?)
        } else {
            None
        };

        self.match_token(TokenKind::EqualToken);
        let initializer = self.parse_expression(0)?;
        self.match_token(TokenKind::SemicolonToken);

        Ok(crate::nodes::GlobalVariableNode {
            name,
            declared_type,
            initializer,
            is_const,
            is_public,
            is_static,
            file_path: None,
        })
    }

    /// Parses formal parameters for a function declaration. A parameter may carry a constant-literal
    /// default value (`name: type = <literal>`); once one parameter has a default, every parameter
    /// after it must also have one (defaults must be trailing).
    pub(super) fn parse_formal_parameters(&mut self) -> Result<Vec<ParameterNode>, Error> {
        let mut params = vec![];
        //eat the open parenthesis
        self.match_token(TokenKind::OpenParenthesisToken);

        let mut seen_default = false;
        while self.current_token().kind != TokenKind::CloseParenthesisToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let index_before = self.current_token_index;
            //eat the identifier
            let param = self.match_token(TokenKind::IdentifierToken);
            //eat the colon
            self.match_token(TokenKind::ColonToken);

            let param_type = self.parse_type()?;

            // Optional default value: `= <literal>`. Restricted to constant literals so no
            // evaluation is needed at the call site.
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

            // Safety: if a malformed parameter consumed no tokens (e.g. a reserved keyword used
            // as a parameter name), advance one token to avoid an infinite loop.
            if self.current_token_index == index_before {
                self.next_token();
            }
            //if we have comma and it is not trailing comma
            if self.current_token().kind == TokenKind::CommaToken {
                //next token of comma is identifier eat comma then
                if self.peek_token(1).kind == TokenKind::IdentifierToken {
                    //eat the comma
                    self.match_token(TokenKind::CommaToken);
                }
            }
        }

        //eat the close parenthesis
        self.match_token(TokenKind::CloseParenthesisToken);
        Ok(params)
    }
}
