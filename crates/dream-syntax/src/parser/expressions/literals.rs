use super::super::Parser;
use crate::lexer::Lexer;
use crate::nodes::{
    ExpressionNode,
    Type,
};
use crate::token::syntax_token::SyntaxToken;
use crate::token::token_kind::TokenKind;
use crate::token::token_kind::TokenKind::EndOfFileToken;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {
    /// Parses a `{...}` Set or Map literal, assuming the cursor is on the opening `{`.
    /// Disambiguated from a statement block by parse position alone (a block never appears where
    /// an expression is expected), and Set vs Map by whether a `:` follows the first element:
    /// `{e1, e2, ...}` is a Set, `{k1: v1, k2: v2, ...}` is a Map. A stray `:` on a later element
    /// of a Set (or a missing one in a Map) falls through to the shared delimited-list parser's
    /// normal token-mismatch recovery, reporting a clear diagnostic rather than silently
    /// misparsing. An empty `{}` is ambiguous between an empty Set and an empty Map, so it is
    /// represented as an empty `SetLiteral`; the analyzer reinterprets it as an empty map when the
    /// surrounding expected type says so (mirroring how an empty array literal `[]` infers its
    /// element type from context).
    pub(crate) fn parse_set_or_map_literal(&mut self) -> Result<ExpressionNode<'a>, Error> {
        self.match_token(TokenKind::CurlyOpenBracketToken);
        if self.current_token().kind == TokenKind::CurlyCloseBracketToken {
            self.match_token(TokenKind::CurlyCloseBracketToken);
            return Ok(ExpressionNode::SetLiteral(vec![]));
        }

        let first = self.parse_expression(0)?;
        if self.current_token().kind == TokenKind::ColonToken {
            self.match_token(TokenKind::ColonToken);
            let first_value = self.parse_expression(0)?;
            let mut entries = vec![(first, first_value)];
            if self.current_token().kind == TokenKind::CommaToken {
                self.match_token(TokenKind::CommaToken);
                let rest = self.parse_delimited_list(TokenKind::CurlyCloseBracketToken, |p| {
                    let key = p.parse_expression(0)?;
                    p.match_token(TokenKind::ColonToken);
                    let value = p.parse_expression(0)?;
                    Ok((key, value))
                })?;
                entries.extend(rest);
            } else {
                self.match_token(TokenKind::CurlyCloseBracketToken);
            }
            Ok(ExpressionNode::MapLiteral(entries))
        } else {
            let mut elements = vec![first];
            if self.current_token().kind == TokenKind::CommaToken {
                self.match_token(TokenKind::CommaToken);
                let rest = self.parse_delimited_list(TokenKind::CurlyCloseBracketToken, |p| {
                    p.parse_expression(0)
                })?;
                elements.extend(rest);
            } else {
                self.match_token(TokenKind::CurlyCloseBracketToken);
            }
            Ok(ExpressionNode::SetLiteral(elements))
        }
    }

    /// Lowers an interpolated string literal `$"...{expr}..."` into the existing string
    /// concatenation chain. `$"{y+68} is {x}"` becomes `"" + (y + 68) + " is " + (x)`, reusing
    /// the analyzer/codegen `string + T` path that auto-converts each non-string operand through
    /// the `to_string` object protocol. The chain is seeded with an empty string literal so the
    /// whole expression is always typed `string`, even for a lone hole like `$"{x}"`.
    pub(crate) fn parse_interpolated_string(
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
    pub(crate) fn make_string_literal(
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
    pub(crate) fn parse_interpolation_hole(
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
