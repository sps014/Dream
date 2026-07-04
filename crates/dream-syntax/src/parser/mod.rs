use crate::lexer::Lexer;
use crate::nodes::{ProgramNode, Type};
use crate::syntax_tree::SyntaxTree;
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use bumpalo::Bump;
use dream_diagnostics::DiagnosticBag;
use dream_text::line_text::LineText;
use dream_text::text_span::TextSpan;
use std::collections::HashMap;
use std::io::Error;

mod declarations;
mod expressions;
mod statements;

/// The parser is responsible for converting a sequence of tokens into an Abstract Syntax Tree (AST).
/// It uses a recursive descent parsing strategy.
pub struct Parser<'a, 'b> {
    lexer: Lexer,
    tokens: Vec<SyntaxToken>,
    current_token_index: usize,
    arena: &'a Bump,
    diagnostics: &'b mut DiagnosticBag,
    /// Monotonic counter used to generate unique synthetic local names for `for-each` lowering.
    foreach_counter: usize,
    /// Declared type aliases (`type Foo = Bar;`). Resolved (erased) at parse time so the rest of
    /// the compiler never sees the alias name.
    type_aliases: HashMap<String, Type>,
}

impl<'a, 'b> Parser<'a, 'b> {
    ///creates a new instance of the parser from a lexer instance
    pub fn new(lexer: Lexer, arena: &'a Bump, diagnostics: &'b mut DiagnosticBag) -> Self {
        Self {
            lexer,
            tokens: Vec::new(),
            current_token_index: 0,
            arena,
            diagnostics,
            foreach_counter: 0,
            type_aliases: HashMap::new(),
        }
    }
    //returns the new eof token
    fn new_eof_token() -> SyntaxToken {
        SyntaxToken::new(
            TokenKind::EndOfFileToken,
            TextSpan::new((0, 0), &LineText::new("".to_string())),
            "\0".to_string(),
        )
    }
    ///returns current token if exists or None
    fn current_token(&self) -> SyntaxToken {
        if self.current_token_index >= self.tokens.len() {
            Parser::new_eof_token()
        } else {
            self.tokens[self.current_token_index].clone()
        }
    }
    ///returns current token and moves to next token
    fn next_token(&mut self) -> SyntaxToken {
        let r = self.current_token();
        // Clamp at the end of the stream so repeated `next_token` calls during error recovery can
        // never push the cursor arbitrarily far past EOF (which previously enabled out-of-bounds
        // indexing). `current_token` keeps returning a synthetic EOF once we reach the end.
        if self.current_token_index < self.tokens.len() {
            self.current_token_index += 1;
        }
        r
    }
    ///return the token at the given index with some offset
    fn peek_token(&self, offset: usize) -> SyntaxToken {
        if self.current_token_index + offset >= self.tokens.len() {
            Parser::new_eof_token()
        } else {
            self.tokens[self.current_token_index + offset].clone()
        }
    }
    ///checks if the current token is of the given kind, returns that token, moves to next token else synthesizes one and reports error
    fn match_token(&mut self, kind: TokenKind) -> SyntaxToken {
        let token = self.current_token();
        if token.kind == kind {
            self.next_token()
        } else {
            let mut err_pos = token.position;
            // If we are looking for a semicolon and we missed it, point the error
            // at the end of the previous token rather than the current token.
            if kind == TokenKind::SemicolonToken {
                // The cursor can run one-or-more tokens past the end of the stream during error
                // recovery, so resolve the previous token with a bounds-checked `get` rather than
                // indexing (which would panic on malformed/truncated input).
                let prev_token = self
                    .current_token_index
                    .checked_sub(1)
                    .and_then(|i| self.tokens.get(i))
                    .cloned()
                    .unwrap_or_else(|| token.clone());

                if prev_token.position.line_no < token.position.line_no
                    || token.kind == TokenKind::EndOfFileToken
                    || token.kind == TokenKind::CurlyCloseBracketToken
                {
                    err_pos = prev_token.position;
                    err_pos.start = err_pos.end;
                    err_pos.col_no += err_pos.end - prev_token.position.start;
                }
            } else {
                err_pos.end = err_pos.start;
            }

            self.diagnostics.report_error(
                format!(
                    "Expected {} but found {}",
                    kind.friendly_name(),
                    token.kind.friendly_name()
                ),
                Some(err_pos),
            );
            SyntaxToken::new(kind, err_pos, "".to_string())
        }
    }
    /// Matches a member/method *name*: an identifier, or any identifier-shaped reserved word
    /// (`object`, `type`, `default`, `in`, ... — all keyword/data-type tokens whose text is a valid
    /// identifier). Reserved words are re-tagged as `IdentifierToken` so downstream analysis treats
    /// them uniformly. This lets dynamic `js` interop access JS members named like Dream keywords
    /// (`el.type`, `x.default`, `js.object()`) and lets such names be declared as methods.
    fn match_member_name(&mut self) -> SyntaxToken {
        let token = self.current_token();
        if token.kind == TokenKind::IdentifierToken {
            return self.next_token();
        }
        let is_word = !token.text.is_empty()
            && token
                .text
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && token
                .text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if is_word && token.kind != TokenKind::EndOfFileToken {
            let mut t = self.next_token();
            t.kind = TokenKind::IdentifierToken;
            t
        } else {
            // Not a name: fall back to the standard identifier error/recovery path.
            self.match_token(TokenKind::IdentifierToken)
        }
    }
    /// True if the current token can close a generic argument list: either a plain `>` or the
    /// first half of a `>>` (`ShiftRightToken`), which appears when two generic lists end
    /// together, e.g. the `>>` in `Box<Box<int>>`.
    fn is_generic_close(&self) -> bool {
        matches!(
            self.current_token().kind,
            TokenKind::GreaterThanToken | TokenKind::ShiftRightToken
        )
    }
    /// Consumes one generic-list closing `>`. When the current token is `>>` it is split in
    /// place: one `>` is consumed conceptually and the pending token is rewritten to a single
    /// `>` so the enclosing generic list can close on the next call. Reports an error if neither
    /// is present.
    fn match_generic_close(&mut self) {
        let token = self.current_token();
        match token.kind {
            TokenKind::GreaterThanToken => {
                self.next_token();
            }
            TokenKind::ShiftRightToken => {
                // Rewrite `>>` to a single `>` and stay put so the outer close consumes it.
                if self.current_token_index < self.tokens.len() {
                    let remaining = &mut self.tokens[self.current_token_index];
                    remaining.kind = TokenKind::GreaterThanToken;
                    remaining.text = ">".to_string();
                }
            }
            _ => {
                self.match_token(TokenKind::GreaterThanToken);
            }
        }
    }
    /// Parses a comma-separated list of generic type arguments, assuming the opening `<`
    /// has already been consumed, and consumes the matching closing `>`/`>>`. Used at every
    /// site that accepts generic arguments (type annotations, function/method calls, struct
    /// instantiation) so the loop and recovery logic live in one place.
    fn parse_generic_args(&mut self) -> Result<Vec<Type>, Error> {
        let mut args = Vec::new();
        while !self.is_generic_close() && self.current_token().kind != TokenKind::EndOfFileToken {
            let iter = self.current_token_index;
            args.push(self.parse_type()?);
            if self.current_token().kind == TokenKind::CommaToken {
                self.match_token(TokenKind::CommaToken);
            }
            self.ensure_progress(iter);
        }
        self.match_generic_close();
        Ok(args)
    }
    /// Parses a comma-separated list of elements terminated by `close`, assuming the opening
    /// delimiter has already been consumed, and consumes the matching `close`. A trailing comma is
    /// permitted and [`ensure_progress`] guards against spinning on malformed input. Centralizes the
    /// ~half-dozen identical "while not close { elem; optional comma } close" loops (array literals,
    /// call arguments, variant fields, pattern args, function-type params, match arms).
    fn parse_delimited_list<T>(
        &mut self,
        close: TokenKind,
        mut parse_elem: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let mut items = Vec::new();
        while self.current_token().kind != close
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            items.push(parse_elem(self)?);
            if self.current_token().kind == TokenKind::CommaToken {
                self.match_token(TokenKind::CommaToken);
            }
            self.ensure_progress(iter);
        }
        self.match_token(close);
        Ok(items)
    }

    /// Parses an optional generic *parameter* declaration list `<T, U, ...>` of bare identifiers,
    /// each optionally carrying interface bounds `T : Iface (+ Iface)*` (the declaration side;
    /// [`parse_generic_args`] parses concrete type *arguments*). Returns `None` when no `<` follows.
    /// Shared by enum/struct/interface/extend/function declarations. The bounds are returned
    /// separately so the caller can attach them (only class/struct, interface, function, and
    /// `extend` carry a `generic_constraints` list).
    fn parse_identifier_generic_params(
        &mut self,
    ) -> Option<(Vec<SyntaxToken>, Vec<crate::nodes::GenericConstraint>)> {
        if self.current_token().kind != TokenKind::SmallerThanToken {
            return None;
        }
        self.match_token(TokenKind::SmallerThanToken);
        let mut params = Vec::new();
        let mut constraints = Vec::new();
        while self.current_token().kind != TokenKind::GreaterThanToken
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            let iter = self.current_token_index;
            let param = self.match_token(TokenKind::IdentifierToken);
            // Optional bounds: `T : Comparable<T>` or `T : Equatable<T> + Comparable<T>`.
            if self.current_token().kind == TokenKind::ColonToken {
                self.match_token(TokenKind::ColonToken);
                let mut bounds = Vec::new();
                if let Ok(first) = self.parse_type() {
                    bounds.push(first);
                }
                while self.current_token().kind == TokenKind::PlusToken {
                    self.match_token(TokenKind::PlusToken);
                    if let Ok(next) = self.parse_type() {
                        bounds.push(next);
                    }
                }
                constraints.push(crate::nodes::GenericConstraint {
                    param: param.clone(),
                    bounds,
                });
            }
            params.push(param);
            if self.current_token().kind == TokenKind::CommaToken {
                self.match_token(TokenKind::CommaToken);
            }
            self.ensure_progress(iter);
        }
        self.match_token(TokenKind::GreaterThanToken);
        Some((params, constraints))
    }

    /// Convenience over [`Self::parse_identifier_generic_params`] that splits the result into the
    /// param-name list (`None` when no `<` follows) and the constraint list (empty otherwise), the
    /// two shapes the declaration nodes store.
    fn take_generic_params(
        &mut self,
    ) -> (
        Option<Vec<SyntaxToken>>,
        Vec<crate::nodes::GenericConstraint>,
    ) {
        match self.parse_identifier_generic_params() {
            Some((params, constraints)) => (Some(params), constraints),
            None => (None, Vec::new()),
        }
    }

    /// Recovery guard for token-consuming loops: if no token has been consumed since `mark`,
    /// skip one token so malformed input surfaces an error (already reported by the failing
    /// `match_token`) instead of spinning forever. Never advances past end-of-file.
    fn ensure_progress(&mut self, mark: usize) {
        if self.current_token_index == mark
            && self.current_token().kind != TokenKind::EndOfFileToken
        {
            self.next_token();
        }
    }
    /// Lookahead over a balanced generic argument list whose first argument token is at peek
    /// offset `start` (i.e. the opening `<` was already seen). Tracks nesting so multi-argument
    /// and nested generics (`Pair<Box<int>, int>`, `Box<Box<int>>`) are handled, treating `>>`
    /// as two closing `>`. Returns the peek offset of the token right after the matching close,
    /// or `None` if a `;`/end-of-file is hit first (not a generic list). Used only to
    /// disambiguate generic calls/instantiations from `<`/`>` comparisons.
    fn scan_generic_args(&self, mut i: usize) -> Option<usize> {
        let mut depth: i32 = 1;
        while self.peek_token(i).kind != TokenKind::EndOfFileToken {
            match self.peek_token(i).kind {
                TokenKind::SmallerThanToken => depth += 1,
                TokenKind::GreaterThanToken => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                TokenKind::ShiftRightToken => {
                    depth -= 2;
                    if depth <= 0 {
                        return Some(i + 1);
                    }
                }
                TokenKind::SemicolonToken => return None,
                _ => {}
            }
            i += 1;
        }
        None
    }
    ///parse all tokens from lexer and returns a syntax tree or error
    pub fn parse(&mut self) -> Result<SyntaxTree<'a>, Error> {
        self.tokens = self.lexer.lex_all(self.diagnostics);
        Ok(SyntaxTree::new(self.parse_program()?))
    }

    /// Returns the kind of the first token at or after the cursor that is not a leading
    /// declaration modifier (`public`, `static`, `async`). Used to classify a top-level
    /// declaration regardless of the order/number of modifiers preceding its core keyword
    /// (e.g. `public static let`, `public async fun`).
    fn first_keyword_after_modifiers(&self) -> TokenKind {
        let mut i = 0;
        loop {
            match self.peek_token(i).kind {
                TokenKind::PublicToken | TokenKind::StaticToken | TokenKind::AsyncToken => i += 1,
                other => return other,
            }
        }
    }

    /// Like [`first_keyword_after_modifiers`], but also skips leading attribute groups
    /// (`@name` optionally followed by a balanced `( ... )`). Used to classify a declaration that
    /// may be preceded by attributes, e.g. `@json enum Shape { ... }`.
    fn core_keyword_after_attrs(&self) -> TokenKind {
        let mut i = 0;
        loop {
            match self.peek_token(i).kind {
                TokenKind::PublicToken | TokenKind::StaticToken | TokenKind::AsyncToken => i += 1,
                TokenKind::AtToken => {
                    i += 1; // `@`
                    if self.peek_token(i).kind == TokenKind::IdentifierToken {
                        i += 1; // attribute name
                    }
                    if self.peek_token(i).kind == TokenKind::OpenParenthesisToken {
                        let mut depth = 1;
                        i += 1;
                        while depth > 0 && self.peek_token(i).kind != TokenKind::EndOfFileToken {
                            match self.peek_token(i).kind {
                                TokenKind::OpenParenthesisToken => depth += 1,
                                TokenKind::CloseParenthesisToken => depth -= 1,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                }
                other => return other,
            }
        }
    }

    ///get all functions in the file
    fn parse_program(&mut self) -> Result<ProgramNode<'a>, Error> {
        let mut imports = vec![];
        let mut functions = vec![];
        let mut structs = vec![];
        let mut interfaces = vec![];
        let mut enums = vec![];
        let mut extends = vec![];
        let mut globals = vec![];

        while self.current_token().kind == TokenKind::ImportToken {
            if let Ok(import_node) = self.parse_import() {
                imports.push(import_node);
            } else {
                self.recover_to_next_declaration();
            }
        }

        while self.current_token().kind != TokenKind::EndOfFileToken {
            let loop_start = self.current_token_index;
            let cur = self.current_token().kind;
            // The core declaration keyword, looking past any leading `public`/`static`/`async`.
            let core = self.first_keyword_after_modifiers();
            if core == TokenKind::ClassToken
                || core == TokenKind::StructToken
                || (cur == TokenKind::AtToken
                    && self.peek_token(1).kind == TokenKind::IdentifierToken
                    && (matches!(
                        self.peek_token(2).kind,
                        TokenKind::ClassToken | TokenKind::StructToken
                    ) || matches!(
                        self.peek_token(3).kind,
                        TokenKind::ClassToken | TokenKind::StructToken
                    )))
            {
                match self.parse_struct_declaration() {
                    Ok(struct_decl) => structs.push(struct_decl),
                    Err(_) => self.recover_to_next_declaration(),
                }
            } else if core == TokenKind::InterfaceToken
                || (cur == TokenKind::AtToken
                    && self.peek_token(1).kind == TokenKind::IdentifierToken
                    && (self.peek_token(2).kind == TokenKind::InterfaceToken
                        || self.peek_token(3).kind == TokenKind::InterfaceToken))
            {
                match self.parse_interface_declaration() {
                    Ok(iface) => interfaces.push(iface),
                    Err(_) => self.recover_to_next_declaration(),
                }
            } else if cur == TokenKind::EnumToken
                || (cur == TokenKind::AtToken
                    && self.core_keyword_after_attrs() == TokenKind::EnumToken)
            {
                match self.parse_enum_declaration() {
                    Ok(enum_decl) => enums.push(enum_decl),
                    Err(_) => self.recover_to_next_declaration(),
                }
            } else if cur == TokenKind::ExtendToken {
                match self.parse_extend_declaration() {
                    Ok(extend_decl) => extends.push(extend_decl),
                    Err(_) => self.recover_to_next_declaration(),
                }
            } else if cur == TokenKind::TypeToken {
                if self.parse_type_alias().is_err() {
                    self.recover_to_next_declaration();
                }
            } else if core == TokenKind::LetToken || core == TokenKind::ConstToken {
                match self.parse_global_variable() {
                    Ok(global) => globals.push(global),
                    Err(_) => self.recover_to_next_declaration(),
                }
            } else if cur == TokenKind::FunToken
                || cur == TokenKind::AtToken
                || cur == TokenKind::ExternToken
                || core == TokenKind::FunToken
                || core == TokenKind::ExternToken
            {
                match self.parse_function(None) {
                    Ok(function) => functions.push(function),
                    Err(_) => self.recover_to_next_declaration(),
                }
            } else {
                let cur = self.current_token();
                self.diagnostics.report_error(
                    format!(
                        "Expected a declaration (function, class, enum, or variable) but found {}",
                        cur.kind.friendly_name()
                    ),
                    Some(cur.position),
                );
                self.next_token();
            }
            // Final guard: every branch above is expected to consume at least one token (directly
            // or via recovery). If a future change ever leaves the cursor parked, skip a token so
            // top-level parsing can never spin forever.
            self.ensure_progress(loop_start);
        }
        Ok(ProgramNode::new(
            imports, structs, interfaces, functions, enums, extends, globals,
        ))
    }

    /// Skips tokens until a recognized top-level declaration keyword is found,
    /// allowing the parser to recover from a bad declaration and continue building the AST.
    fn recover_to_next_declaration(&mut self) {
        while self.current_token().kind != TokenKind::EndOfFileToken {
            let kind = self.current_token().kind;
            if matches!(
                kind,
                TokenKind::ClassToken
                    | TokenKind::StructToken
                    | TokenKind::InterfaceToken
                    | TokenKind::EnumToken
                    | TokenKind::ExtendToken
                    | TokenKind::FunToken
                    | TokenKind::PublicToken
                    | TokenKind::ExternToken
                    | TokenKind::AsyncToken
                    | TokenKind::TypeToken
                    | TokenKind::LetToken
                    | TokenKind::ConstToken
            ) {
                break;
            }
            self.next_token();
        }
    }
}

#[cfg(test)]
#[path = "../tests/parser_tests.rs"]
mod tests;
