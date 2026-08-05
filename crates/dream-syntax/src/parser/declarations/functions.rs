use super::super::Parser;
use crate::nodes::{
    FunctionNode, ParameterNode, StatementNode, Type,
    Visibility,
};
use crate::token::token_kind::TokenKind;
use std::io::Error;

/// The four modifiers a `fun`/`constructor`/`del` declaration may carry, parsed from the flexible
/// `async`/`public`/`internal`/`static`/`extern` prefix (which may appear in several orders).
#[derive(Default)]
pub(crate) struct FunctionModifiers {
    pub(crate) is_async: bool,
    pub(crate) visibility: Visibility,
    pub(crate) is_static: bool,
    pub(crate) is_extern: bool,
}

impl<'a, 'b> Parser<'a, 'b> {

    /// Parses the flexible function-modifier prefix (`async`/`public`/`internal`/`static`/`extern`,
    /// which may appear in several orders) and reports the `public`+`extern` conflict. Consumes
    /// exactly the modifier tokens, leaving the cursor on the `fun`/constructor/`del` token.
    pub(crate) fn parse_function_modifiers(&mut self) -> FunctionModifiers {
        let mut m = FunctionModifiers::default();

        // `async` may appear before or after `public`/`internal` (e.g. `async fun`, `public async
        // fun`, `async public fun`). Calling such a function eagerly starts a task and yields
        // `Future<T>`.
        if self.current_token().kind == TokenKind::AsyncToken {
            self.match_token(TokenKind::AsyncToken);
            m.is_async = true;
        }

        self.try_consume_visibility(&mut m.visibility);

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
            if m.visibility.is_public() {
                self.diagnostics.report_error(
                    "A function cannot be both 'public' and 'extern': 'extern' declares an imported host symbol, while 'public' exports a defined one".to_string(),
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
    pub(crate) fn parse_function(
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
            visibility,
            is_static,
            is_extern,
        } = self.parse_function_modifiers();

        // Constructor (`constructor`) / destructor (`del`) declarations omit the `fun` keyword and
        // the return type; they are lowered to ordinary methods named `constructor`/`del` and
        // dispatched specially (constructor calls, scope-exit destructor calls). They cannot be
        // marked `public`/`internal`.
        if self.current_token().kind == TokenKind::IdentifierToken
            && crate::nodes::types::is_special_member_name(&self.current_token().text)
        {
            let ctor_name = self.match_token(TokenKind::IdentifierToken);
            if visibility != Visibility::Private {
                self.diagnostics.report_error(
                    format!(
                        "'{}' cannot be marked 'public' or 'internal'",
                        ctor_name.text
                    ),
                    Some(ctor_name.position),
                );
            }
            let params = self.parse_formal_parameters()?;
            let block = self.parse_block()?;
            return Ok(FunctionNode::new(
                attributes,
                ctor_name,
                None,
                None,
                params,
                block,
                Visibility::Private,
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
                visibility,
            );
            node.is_static = is_static;
            node.is_async = is_async;
            node.accessor = Some(accessor_kind);
            return Ok(node);
        }

        //eat the fun keyword
        self.match_token(TokenKind::FunToken);
        let mut function_name = self.match_member_name();
        Self::splice_leading_trivia(&mut function_name, first_trivia);

        let (generic_parameters, generic_constraints) = self.take_generic_params();

        let params = self.parse_formal_parameters()?;
        let mut return_type: Option<Type> = None;
        if self.current_token().kind == TokenKind::ColonToken {
            //eat the colon
            self.match_token(TokenKind::ColonToken);
            return_type = Some(self.parse_type()?);
        }

        let where_constraints = self.parse_where_constraints();

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
                Visibility::Private,
            );
            node.is_extern = true;
            node.is_static = is_static;
            node.is_async = is_async;
            node.generic_constraints = generic_constraints;
            node.where_constraints = where_constraints;
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
            visibility,
        );
        node.is_static = is_static;
        node.is_async = is_async;
        node.generic_constraints = generic_constraints;
        node.where_constraints = where_constraints;
        Ok(node)
    }

    /// Parses formal parameters for a function declaration. A parameter may carry a constant-literal
    /// default value (`name: type = <literal>`); once one parameter has a default, every parameter
    /// after it must also have one (defaults must be trailing).
    pub(crate) fn parse_formal_parameters(&mut self) -> Result<Vec<ParameterNode>, Error> {
        let mut params = vec![];
        //eat the open parenthesis
        self.match_token(TokenKind::OpenParenthesisToken);

        let mut seen_default = false;
        let mut seen_variadic = false;
        while self.current_token().kind != TokenKind::CloseParenthesisToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let index_before = self.current_token_index;

            // A `ref name: T` parameter: the callee shares the caller's storage. Mutually
            // exclusive with variadic/default, enforced below.
            let is_ref = self.current_token().kind == TokenKind::RefToken;
            if is_ref {
                self.match_token(TokenKind::RefToken);
            }

            // A trailing variadic parameter: `...name: T[]`. Must be the last parameter and
            // carries no default (an omitted variadic simply collects zero elements).
            let is_variadic = self.current_token().kind == TokenKind::DotDotDotToken;
            if is_variadic {
                self.match_token(TokenKind::DotDotDotToken);
            }

            //eat the identifier
            let param = self.match_token(TokenKind::IdentifierToken);
            //eat the colon
            self.match_token(TokenKind::ColonToken);

            let param_type = self.parse_type()?;

            if is_ref && is_variadic {
                self.diagnostics.report_error(
                    format!(
                        "parameter '{}' cannot be both 'ref' and variadic",
                        param.text
                    ),
                    Some(param.position),
                );
            }

            if is_variadic {
                if seen_variadic {
                    self.diagnostics.report_error(
                        "a function may have at most one variadic parameter".to_string(),
                        Some(param.position),
                    );
                }
                if !matches!(param_type, Type::Array(_)) {
                    self.diagnostics.report_error(
                        format!(
                            "variadic parameter '{}' must have an array type, e.g. '...{}: T[]'",
                            param.text, param.text
                        ),
                        Some(param.position),
                    );
                }
                seen_variadic = true;
                params.push(ParameterNode::variadic(param, param_type));
            } else {
                if seen_variadic {
                    self.diagnostics.report_error(
                        format!(
                            "parameter '{}' cannot follow the variadic parameter; the variadic parameter must be last",
                            param.text
                        ),
                        Some(param.position),
                    );
                }
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
                }
            }

            // Safety: if a malformed parameter consumed no tokens (e.g. a reserved keyword used
            // as a parameter name), advance one token to avoid an infinite loop.
            if self.current_token_index == index_before {
                self.next_token();
            }
            //if we have comma and it is not trailing comma
            if self.current_token().kind == TokenKind::CommaToken {
                //next token of comma is identifier (or `...`/`ref` starting the next parameter) eat comma then
                if matches!(
                    self.peek_token(1).kind,
                    TokenKind::IdentifierToken | TokenKind::DotDotDotToken | TokenKind::RefToken
                ) {
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
