use super::super::Parser;
use crate::nodes::{
    ImportNode, ModuleDeclNode,
};
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {

    /// Parses an import statement. Two forms, told apart by a trailing `as` clause:
    /// - `import a.b.c;` — maps each dotted segment to a directory separator (`a/b/c`) so file
    ///   resolution can append the `.dream` extension later, exactly as before `as` existed.
    /// - `import a.b.c as x;` — keeps the path dot-joined (`a.b.c`): it names an item inside a
    ///   *declared* `module` namespace, not a file path, resolved separately after all files load.
    pub(crate) fn parse_import(&mut self) -> Result<ImportNode, Error> {
        self.match_token(TokenKind::ImportToken);

        let first = self.match_token(TokenKind::IdentifierToken);
        let mut position = first.position;
        let mut slash_path = first.text.clone();
        let mut dot_path = first.text.clone();

        while self.current_token().kind == TokenKind::DotToken {
            self.match_token(TokenKind::DotToken);
            let segment = self.match_token(TokenKind::IdentifierToken);
            position.end = segment.position.end;
            slash_path.push('/');
            slash_path.push_str(&segment.text);
            dot_path.push('.');
            dot_path.push_str(&segment.text);
        }

        if self.current_token().kind == TokenKind::AsToken {
            self.match_token(TokenKind::AsToken);
            let alias = self.match_token(TokenKind::IdentifierToken);
            self.match_token(TokenKind::SemicolonToken);
            let module_path = SyntaxToken::new(TokenKind::IdentifierToken, position, dot_path);
            return Ok(ImportNode::with_alias(module_path, alias));
        }

        self.match_token(TokenKind::SemicolonToken);

        let module_name = SyntaxToken::new(TokenKind::IdentifierToken, position, slash_path);
        Ok(ImportNode::new(module_name))
    }

    /// Parses a file-scoped `module a.b.c;` declaration. Must be the first item in the file
    /// (checked by the caller, `parse_program`); at most one per file (checked by the caller too,
    /// since a second occurrence would otherwise just parse as an ordinary statement-position
    /// error). The path is kept dot-joined, mirroring the `module` namespaces it names (never a
    /// filesystem path, unlike a plain `import`).
    pub(crate) fn parse_module_decl(&mut self) -> Result<ModuleDeclNode, Error> {
        self.match_token(TokenKind::ModuleToken);

        let first = self.match_token(TokenKind::IdentifierToken);
        let mut position = first.position;
        let mut path = first.text.clone();

        while self.current_token().kind == TokenKind::DotToken {
            self.match_token(TokenKind::DotToken);
            let segment = self.match_token(TokenKind::IdentifierToken);
            position.end = segment.position.end;
            path.push('.');
            path.push_str(&segment.text);
        }

        self.match_token(TokenKind::SemicolonToken);

        let path = SyntaxToken::new(TokenKind::IdentifierToken, position, path);
        Ok(ModuleDeclNode {
            attributes: Vec::new(),
            path,
        })
    }

    /// Parses `@attrs module a.b.c;` — attributes are collected by the caller and passed in.
    pub(crate) fn parse_module_decl_with_attrs(
        &mut self,
        attributes: Vec<crate::nodes::AttributeNode>,
    ) -> Result<ModuleDeclNode, Error> {
        let mut decl = self.parse_module_decl()?;
        decl.attributes = attributes;
        Ok(decl)
    }
}
