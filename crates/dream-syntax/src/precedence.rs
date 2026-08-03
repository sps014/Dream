use crate::token::token_kind::TokenKind;

impl TokenKind {
    pub fn get_binary_precedence(&self) -> i32 {
        match self {
            TokenKind::BitWiseAmpersandToken => 90,
            TokenKind::BitWiseXorToken => 85,
            TokenKind::BitWisePipeToken => 80,

            TokenKind::ModulusToken => 55,

            TokenKind::SlashToken => 50,
            TokenKind::StarToken => 50,

            TokenKind::PlusToken => 40,
            TokenKind::MinusToken => 40,

            TokenKind::ShiftLeftToken => 35,
            TokenKind::ShiftRightToken => 35,

            TokenKind::BangToken => 30,

            TokenKind::GreaterThanEqualToken => 25,
            TokenKind::GreaterThanToken => 25,
            TokenKind::SmallerThanEqualToken => 25,
            TokenKind::SmallerThanToken => 25,
            TokenKind::EqualEqualToken => 25,
            TokenKind::NotEqualToken => 25,
            TokenKind::IsToken => 25,
            // Comparisons/`is` must bind *tighter* than `&&`, which in turn must bind tighter than
            // `||`, so `a > 0 && b > 0` parses as `(a > 0) && (b > 0)` rather than `a > (0 && b) > 0`
            // (the previous ordering here — comparisons at 15, `&&` at 20 — inverted this and made
            // any bare `cond && comparison` a compile error unless parenthesized).
            TokenKind::AmpersandAmpersandToken => 20,
            TokenKind::PipePipeToken => 10,
            TokenKind::QuestionQuestionToken => 8,

            _ => 0,
        }
    }
    pub fn get_unary_precedence(&self) -> i32 {
        match self {
            TokenKind::PlusToken => 6,
            TokenKind::MinusToken => 6,
            TokenKind::BangToken => 6,
            _ => 0,
        }
    }
}
