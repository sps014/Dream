//! Parsing of `name { ... }` syntax-DSL blocks with `{expr}` splices.

use super::super::Parser;
use crate::nodes::{ExpressionNode, SyntaxBlockNode, SyntaxBlockPart};
use crate::token::token_kind::TokenKind;
use dream_text::text_span::TextSpan;
use std::io::Error;

impl<'a, 'b> Parser<'a, 'b> {
    /// Parses `name { parts }` where `parts` are raw text segments and `{ dream_expr }` splices.
    /// Cursor starts on the introducer identifier.
    pub(crate) fn parse_syntax_block(&mut self) -> Result<ExpressionNode<'a>, Error> {
        let name = self.match_token(TokenKind::IdentifierToken);
        let open = self.match_token(TokenKind::CurlyOpenBracketToken);
        let (parts, close_end) = self.parse_syntax_block_parts()?;
        let block_span = TextSpan::new((open.position.start, close_end), self.lexer.line_text().as_ref());
        let node = SyntaxBlockNode {
            name,
            block_span,
            parts,
        };
        Ok(ExpressionNode::SyntaxBlock(self.arena.alloc(node)))
    }

    /// Body of a syntax block until the matching `}` at depth 1. Text is reconstructed from token
    /// slices (with single spaces between non-adjacent tokens) so generators can re-parse markup.
    fn parse_syntax_block_parts(
        &mut self,
    ) -> Result<(Vec<SyntaxBlockPart<'a>>, usize), Error> {
        let mut parts: Vec<SyntaxBlockPart<'a>> = Vec::new();
        let mut text_buf = String::new();
        let mut last_end: Option<usize> = None;

        loop {
            let cur = self.current_token();
            if cur.kind == TokenKind::EndOfFileToken {
                self.diagnostics.report_error(
                    "unclosed syntax block; expected `}`".to_string(),
                    Some(cur.position),
                );
                break;
            }
            if cur.kind == TokenKind::CurlyCloseBracketToken {
                let end = cur.position.end;
                self.next_token();
                if !text_buf.is_empty() {
                    parts.push(SyntaxBlockPart::Text(std::mem::take(&mut text_buf)));
                }
                return Ok((parts, end));
            }
            if cur.kind == TokenKind::CurlyOpenBracketToken {
                if !text_buf.is_empty() {
                    parts.push(SyntaxBlockPart::Text(std::mem::take(&mut text_buf)));
                }
                last_end = None;
                self.next_token(); // `{`
                let expr = self.parse_expression(0)?;
                self.match_token(TokenKind::CurlyCloseBracketToken);
                parts.push(SyntaxBlockPart::Splice(self.arena.alloc(expr)));
                continue;
            }

            // Preserve approximate spacing from source spans when tokens are non-adjacent.
            if let Some(prev_end) = last_end {
                if cur.position.start > prev_end {
                    let gap = cur.position.start - prev_end;
                    if gap > 0 {
                        // Prefer a single space for typical whitespace gaps; keep larger gaps as spaces.
                        text_buf.push_str(&" ".repeat(gap.min(8)));
                    }
                }
            } else if !text_buf.is_empty() {
                text_buf.push(' ');
            }
            text_buf.push_str(&cur.text);
            last_end = Some(cur.position.end);
            self.next_token();
        }

        if !text_buf.is_empty() {
            parts.push(SyntaxBlockPart::Text(std::mem::take(&mut text_buf)));
        }
        Ok((parts, self.current_token().position.end))
    }
}
