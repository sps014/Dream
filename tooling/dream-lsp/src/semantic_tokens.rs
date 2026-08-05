//! Computes LSP semantic tokens by lexing the document and classifying each identifier against
//! the symbol [`crate::index::Index`] (so a name colours as the function/struct/field/etc. it
//! actually refers to), then delta-encoding the result as the protocol requires.

use dream::diagnostics::DiagnosticBag;
use dream::syntax::lexer::Lexer;
use dream::syntax::token::syntax_trivia::SyntaxTrivia;
use dream::syntax::token::token_kind::TokenKind;
use tower_lsp::lsp_types::{SemanticToken, SemanticTokenType};

use crate::index::{Index, SymKind};
use crate::position::LineIndex;
use crate::tokens::{lex_category, LexCategory};

/// The ordered semantic-token legend advertised in the server capabilities. A token's
/// `token_type` is an index into this slice.
pub const TOKEN_TYPES: [SemanticTokenType; 15] = [
    SemanticTokenType::KEYWORD,     // 0
    SemanticTokenType::VARIABLE,    // 1
    SemanticTokenType::PROPERTY,    // 2
    SemanticTokenType::FUNCTION,    // 3
    SemanticTokenType::METHOD,      // 4
    SemanticTokenType::CLASS,       // 5
    SemanticTokenType::ENUM,        // 6
    SemanticTokenType::ENUM_MEMBER, // 7
    SemanticTokenType::PARAMETER,   // 8
    SemanticTokenType::TYPE,        // 9
    SemanticTokenType::OPERATOR,    // 10
    SemanticTokenType::STRING,      // 11
    SemanticTokenType::NUMBER,      // 12
    SemanticTokenType::COMMENT,     // 13
    SemanticTokenType::DECORATOR,   // 14 — `@json`, `@get`, …
];

const COMMENT: u32 = 13;
const DECORATOR: u32 = 14;
const OPERATOR: u32 = 10;

/// Index of a symbol kind into [`TOKEN_TYPES`].
fn sym_kind_token_index(kind: SymKind) -> u32 {
    match kind {
        SymKind::Function => 3,
        SymKind::Struct => 5,
        SymKind::Enum => 6,
        SymKind::EnumMember => 7,
        SymKind::Field => 2,
        SymKind::Method => 4,
        SymKind::Variable => 1,
        SymKind::Param => 8,
        SymKind::Type => 9,
        SymKind::Keyword => 0,
        SymKind::Decorator => DECORATOR,
    }
}

fn push_trivia_comments(
    trivia: &[SyntaxTrivia],
    line_index: &LineIndex,
    out: &mut Vec<(u32, u32, u32, u32)>,
) {
    for t in trivia {
        if !matches!(
            t.kind,
            TokenKind::LineCommentToken | TokenKind::BlockCommentToken
        ) {
            continue;
        }
        // Multi-line block comments: emit one token per line so delta-encoding stays valid.
        let mut offset = t.position.start;
        for (i, line) in t.text.split('\n').enumerate() {
            if line.is_empty() && i + 1 == t.text.split('\n').count() {
                break;
            }
            let len = line.chars().count() as u32;
            if len == 0 {
                offset += 1; // the newline
                continue;
            }
            let start_pos = line_index.position(offset);
            out.push((start_pos.line, start_pos.character, len, COMMENT));
            offset += line.len() + 1;
        }
    }
}

pub fn compute(file_path: Option<&str>, text: &str) -> Vec<SemanticToken> {
    let mut scratch = DiagnosticBag::new(None);
    let mut lexer = Lexer::new(text.to_string());
    let tokens = lexer.lex_all(&mut scratch);
    let idx = Index::build(file_path, text);
    let line_index = LineIndex::new(text);

    let mut semantic_tokens = Vec::new();
    let mut prev_was_at = false;

    for token in tokens {
        push_trivia_comments(&token.leading_trivia, &line_index, &mut semantic_tokens);

        if token.kind != TokenKind::EndOfFileToken && token.kind != TokenKind::BadToken {
            let token_type_index = match token.kind {
                TokenKind::AtToken => {
                    prev_was_at = true;
                    Some(OPERATOR)
                }
                TokenKind::IdentifierToken => {
                    let kind = if prev_was_at {
                        prev_was_at = false;
                        DECORATOR
                    } else if token.text == "this" {
                        0 // keyword
                    } else if let Some(decl) =
                        idx.decls.iter().find(|d| d.start == token.position.start)
                    {
                        sym_kind_token_index(decl.kind)
                    } else if let Some(r) =
                        idx.refs.iter().find(|r| r.start == token.position.start)
                    {
                        sym_kind_token_index(r.kind)
                    } else {
                        1 // variable
                    };
                    Some(kind)
                }
                other => {
                    prev_was_at = false;
                    lex_category(other).map(|c| match c {
                        LexCategory::Keyword => 0,
                        LexCategory::Type => 9,
                        LexCategory::Operator => OPERATOR,
                        LexCategory::String => 11,
                        LexCategory::Number => 12,
                    })
                }
            };

            if let Some(type_idx) = token_type_index {
                if !token.text.contains('\n') {
                    let start_pos = line_index.position(token.position.start);
                    semantic_tokens.push((
                        start_pos.line,
                        start_pos.character,
                        token.text.chars().count() as u32,
                        type_idx,
                    ));
                }
            }
        } else {
            prev_was_at = false;
        }

        push_trivia_comments(&token.trailing_trivia, &line_index, &mut semantic_tokens);
    }

    // Stable sort by line, then char to delta encode
    semantic_tokens.sort_by_key(|t| (t.0, t.1));

    let mut result = Vec::new();
    let mut pre_line = 0;
    let mut pre_char = 0;

    for (line, char, len, type_idx) in semantic_tokens {
        let delta_line = line - pre_line;
        let delta_start = if delta_line == 0 {
            char - pre_char
        } else {
            char
        };

        result.push(SemanticToken {
            delta_line,
            delta_start,
            length: len,
            token_type: type_idx,
            token_modifiers_bitset: 0,
        });
        pre_line = line;
        pre_char = char;
    }

    result
}
