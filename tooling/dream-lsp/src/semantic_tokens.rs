//! Computes LSP semantic tokens by lexing the document and classifying each identifier against
//! the symbol [`crate::index::Index`] (so a name colours as the function/struct/field/etc. it
//! actually refers to), then delta-encoding the result as the protocol requires.

use dream::diagnostics::DiagnosticBag;
use dream::syntax::lexer::Lexer;
use dream::syntax::token::token_kind::TokenKind;
use tower_lsp::lsp_types::{SemanticToken, SemanticTokenType};

use crate::index::{Index, SymKind};
use crate::position::LineIndex;

/// The ordered semantic-token legend advertised in the server capabilities. A token's
/// `token_type` is an index into this slice.
pub const TOKEN_TYPES: [SemanticTokenType; 14] = [
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
];

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
    }
}

pub fn compute(file_path: Option<&str>, text: &str) -> Vec<SemanticToken> {
    let mut scratch = DiagnosticBag::new(None);
    let mut lexer = Lexer::new(text.to_string());
    let tokens = lexer.lex_all(&mut scratch);
    let idx = Index::build(file_path, text);
    let line_index = LineIndex::new(text);

    let mut semantic_tokens = Vec::new();

    for token in tokens {
        if token.kind != TokenKind::EndOfFileToken && token.kind != TokenKind::BadToken {
            let token_type_index = match token.kind {
                TokenKind::IdentifierToken => {
                    let mut kind = 1; // Default to variable
                    if token.text == "this" {
                        kind = 0; // keyword
                    } else if let Some(decl) =
                        idx.decls.iter().find(|d| d.start == token.position.start)
                    {
                        kind = sym_kind_token_index(decl.kind);
                    } else if let Some(r) =
                        idx.refs.iter().find(|r| r.start == token.position.start)
                    {
                        kind = sym_kind_token_index(r.kind);
                    }
                    Some(kind)
                }
                TokenKind::NumberToken => Some(12),
                TokenKind::StringToken
                | TokenKind::InterpolatedStringToken
                | TokenKind::CharToken => Some(11),
                TokenKind::BooleanToken | TokenKind::NullToken => Some(0),
                TokenKind::IfToken
                | TokenKind::ElseToken
                | TokenKind::ForToken
                | TokenKind::WhileToken
                | TokenKind::DoToken
                | TokenKind::ReturnToken
                | TokenKind::BreakToken
                | TokenKind::ContinueToken
                | TokenKind::LetToken
                | TokenKind::ConstToken
                | TokenKind::FunToken
                | TokenKind::StaticToken
                | TokenKind::ImportToken
                | TokenKind::PublicToken
                | TokenKind::ExternToken
                | TokenKind::ClassToken
                | TokenKind::ExtendToken
                | TokenKind::IsToken
                | TokenKind::InToken
                | TokenKind::EnumToken
                | TokenKind::TypeToken
                | TokenKind::SwitchToken
                | TokenKind::CaseToken
                | TokenKind::DefaultToken
                | TokenKind::AsyncToken
                | TokenKind::AwaitToken
                | TokenKind::SealedToken => Some(0),
                TokenKind::DataTypeToken => Some(9),
                TokenKind::PlusToken
                | TokenKind::MinusToken
                | TokenKind::SlashToken
                | TokenKind::StarToken
                | TokenKind::BangToken
                | TokenKind::ModulusToken
                | TokenKind::PlusEqualToken
                | TokenKind::MinusEqualToken
                | TokenKind::StarEqualToken
                | TokenKind::SlashEqualToken
                | TokenKind::ModulusEqualToken
                | TokenKind::PlusPlusToken
                | TokenKind::MinusMinusToken
                | TokenKind::EqualEqualToken
                | TokenKind::NotEqualToken
                | TokenKind::AmpersandAmpersandToken
                | TokenKind::PipePipeToken
                | TokenKind::BitWisePipeToken
                | TokenKind::BitWiseAmpersandToken
                | TokenKind::BitWiseXorToken
                | TokenKind::ShiftLeftToken
                | TokenKind::ShiftRightToken
                | TokenKind::QuestionQuestionToken
                | TokenKind::EqualToken
                | TokenKind::GreaterThanEqualToken
                | TokenKind::GreaterThanToken
                | TokenKind::SmallerThanToken
                | TokenKind::SmallerThanEqualToken => Some(10),
                _ => None,
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
        }
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
