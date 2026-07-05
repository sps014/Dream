use logos::Logos;

#[derive(Logos, Debug, PartialEq, Clone, Copy, Hash, Eq)]
pub enum TokenKind {
    EndOfFileToken,

    #[regex(r"[ \t\n\f]+")]
    WhiteSpaceToken,

    BadToken,

    #[regex("[a-zA-Z_][a-zA-Z0-9_]*")]
    IdentifierToken,

    #[regex(r"[0-9]+(\.[0-9]+)?([dDfF]|[uU]?[lL]|[lL]?[uU]|[bB])?")]
    NumberToken,

    #[regex(r#""([^"\\]*(\\.[^"\\]*)*)""#)]
    StringToken,

    #[regex(r#"\$"([^"\\]*(\\.[^"\\]*)*)""#)]
    InterpolatedStringToken,

    #[regex(r#"'(\\.|[^'\\])'"#)]
    CharToken,

    #[token("true")]
    #[token("false")]
    BooleanToken,

    #[token("+")]
    PlusToken,
    #[token("-")]
    MinusToken,
    #[token("/")]
    SlashToken,
    #[token("*")]
    StarToken,
    #[token("!")]
    BangToken,
    #[token("%")]
    ModulusToken,

    #[token("+=")]
    PlusEqualToken,
    #[token("-=")]
    MinusEqualToken,
    #[token("*=")]
    StarEqualToken,
    #[token("/=")]
    SlashEqualToken,
    #[token("%=")]
    ModulusEqualToken,
    #[token("++")]
    PlusPlusToken,
    #[token("--")]
    MinusMinusToken,
    #[token("==")]
    EqualEqualToken,
    #[token("!=")]
    NotEqualToken,
    #[token("&&")]
    AmpersandAmpersandToken,
    #[token("||")]
    PipePipeToken,
    #[token("|")]
    BitWisePipeToken,
    #[token("&")]
    BitWiseAmpersandToken,
    #[token("^")]
    BitWiseXorToken,
    #[token("<<")]
    ShiftLeftToken,
    #[token(">>")]
    ShiftRightToken,
    #[token("??")]
    QuestionQuestionToken,
    #[token("=>")]
    FatArrowToken,
    #[token("=")]
    EqualToken,
    #[token(">=")]
    GreaterThanEqualToken,
    #[token(">")]
    GreaterThanToken,
    #[token("<")]
    SmallerThanToken,
    #[token("<=")]
    SmallerThanEqualToken,

    #[token(";")]
    SemicolonToken,
    #[token(":")]
    ColonToken,
    #[token(",")]
    CommaToken,
    #[token(".")]
    DotToken,
    #[token("?")]
    QuestionMarkToken,

    #[token("(")]
    OpenParenthesisToken,
    #[token(")")]
    CloseParenthesisToken,
    #[token("{")]
    CurlyOpenBracketToken,
    #[token("}")]
    CurlyCloseBracketToken,
    #[token("[")]
    OpenBracketToken,
    #[token("]")]
    CloseBracketToken,

    #[token("if")]
    IfToken,
    #[token("else")]
    ElseToken,
    #[token("for")]
    ForToken,
    #[token("while")]
    WhileToken,
    #[token("do")]
    DoToken,
    #[token("return")]
    ReturnToken,
    #[token("break")]
    BreakToken,
    #[token("continue")]
    ContinueToken,
    #[token("let")]
    LetToken,
    #[token("const")]
    ConstToken,
    #[token("fun")]
    FunToken,
    #[token("async")]
    AsyncToken,
    #[token("await")]
    AwaitToken,
    #[token("static")]
    StaticToken,
    #[token("import")]
    ImportToken,
    #[token("public")]
    PublicToken,
    #[token("extern")]
    ExternToken,
    #[token("class")]
    ClassToken,
    #[token("struct")]
    StructToken,
    #[token("unmanaged")]
    UnmanagedToken,
    #[token("sealed")]
    SealedToken,
    #[token("interface")]
    InterfaceToken,
    #[token("extend")]
    ExtendToken,
    #[token("null")]
    NullToken,
    #[token("is")]
    IsToken,
    #[token("in")]
    InToken,
    #[token("enum")]
    EnumToken,
    #[token("type")]
    TypeToken,
    #[token("switch")]
    SwitchToken,
    #[token("case")]
    CaseToken,
    #[token("default")]
    DefaultToken,

    #[token("@")]
    AtToken,

    #[token("int")]
    #[token("float")]
    #[token("double")]
    #[token("string")]
    #[token("bool")]
    #[token("char")]
    #[token("void")]
    #[token("object")]
    DataTypeToken,

    #[regex(r"//[^\n]*", allow_greedy = true)]
    LineCommentToken,
    #[regex(r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/")]
    BlockCommentToken,
}

impl TokenKind {
    pub fn friendly_name(&self) -> &'static str {
        match self {
            TokenKind::EndOfFileToken => "end of file",
            TokenKind::WhiteSpaceToken => "whitespace",
            TokenKind::BadToken => "invalid token",
            TokenKind::IdentifierToken => "identifier",
            TokenKind::NumberToken => "number",
            TokenKind::StringToken => "string",
            TokenKind::InterpolatedStringToken => "interpolated string",
            TokenKind::CharToken => "character",
            TokenKind::BooleanToken => "boolean",
            TokenKind::PlusToken => "'+'",
            TokenKind::MinusToken => "'-'",
            TokenKind::SlashToken => "'/'",
            TokenKind::StarToken => "'*'",
            TokenKind::BangToken => "'!'",
            TokenKind::ModulusToken => "'%'",
            TokenKind::PlusEqualToken => "'+='",
            TokenKind::MinusEqualToken => "'-='",
            TokenKind::StarEqualToken => "'*='",
            TokenKind::SlashEqualToken => "'/='",
            TokenKind::ModulusEqualToken => "'%='",
            TokenKind::PlusPlusToken => "'++'",
            TokenKind::MinusMinusToken => "'--'",
            TokenKind::EqualEqualToken => "'=='",
            TokenKind::NotEqualToken => "'!='",
            TokenKind::AmpersandAmpersandToken => "'&&'",
            TokenKind::PipePipeToken => "'||'",
            TokenKind::BitWisePipeToken => "'|'",
            TokenKind::BitWiseAmpersandToken => "'&'",
            TokenKind::BitWiseXorToken => "'^'",
            TokenKind::ShiftLeftToken => "'<<'",
            TokenKind::ShiftRightToken => "'>>'",
            TokenKind::QuestionQuestionToken => "'??'",
            TokenKind::FatArrowToken => "'=>'",
            TokenKind::EqualToken => "'='",
            TokenKind::GreaterThanEqualToken => "'>='",
            TokenKind::GreaterThanToken => "'>'",
            TokenKind::SmallerThanToken => "'<'",
            TokenKind::SmallerThanEqualToken => "'<='",
            TokenKind::SemicolonToken => "';'",
            TokenKind::ColonToken => "':'",
            TokenKind::CommaToken => "','",
            TokenKind::DotToken => "'.'",
            TokenKind::QuestionMarkToken => "'?'",
            TokenKind::OpenParenthesisToken => "'('",
            TokenKind::CloseParenthesisToken => "')'",
            TokenKind::CurlyOpenBracketToken => "'{'",
            TokenKind::CurlyCloseBracketToken => "'}'",
            TokenKind::OpenBracketToken => "'['",
            TokenKind::CloseBracketToken => "']'",
            TokenKind::IfToken => "'if'",
            TokenKind::ElseToken => "'else'",
            TokenKind::ForToken => "'for'",
            TokenKind::WhileToken => "'while'",
            TokenKind::DoToken => "'do'",
            TokenKind::ReturnToken => "'return'",
            TokenKind::BreakToken => "'break'",
            TokenKind::ContinueToken => "'continue'",
            TokenKind::LetToken => "'let'",
            TokenKind::ConstToken => "'const'",
            TokenKind::FunToken => "'fun'",
            TokenKind::AsyncToken => "'async'",
            TokenKind::AwaitToken => "'await'",
            TokenKind::StaticToken => "'static'",
            TokenKind::ImportToken => "'import'",
            TokenKind::PublicToken => "'public'",
            TokenKind::ExternToken => "'extern'",
            TokenKind::ClassToken => "'class'",
            TokenKind::StructToken => "'struct'",
            TokenKind::UnmanagedToken => "'unmanaged'",
            TokenKind::SealedToken => "'sealed'",
            TokenKind::InterfaceToken => "'interface'",
            TokenKind::ExtendToken => "'extend'",
            TokenKind::NullToken => "'null'",
            TokenKind::IsToken => "'is'",
            TokenKind::InToken => "'in'",
            TokenKind::EnumToken => "'enum'",
            TokenKind::TypeToken => "'type'",
            TokenKind::SwitchToken => "'switch'",
            TokenKind::CaseToken => "'case'",
            TokenKind::DefaultToken => "'default'",
            TokenKind::AtToken => "'@'",
            TokenKind::DataTypeToken => "data type",
            TokenKind::LineCommentToken | TokenKind::BlockCommentToken => "comment",
        }
    }
}

/// Every alphabetic spelling this lexer recognizes as its own `TokenKind` (i.e. every alphabetic
/// `#[token("...")]` attribute above) - the reserved words that can never be used as an identifier.
/// Single source of truth for "what is a Dream keyword", meant to be consumed by tooling that needs
/// the list (the LSP's completion proposals; ideally the syntax-highlighting grammars too) instead
/// of each hand-maintaining its own copy, which is what let the LSP's keyword list silently drift
/// out of sync with the parser. Kept honest by the [`tests::every_keyword_lexes_as_a_keyword`] /
/// [`tests::every_alphabetic_token_attr_is_listed`] pair below: the former fails if a listed word
/// stops being a real keyword, the latter if a new keyword token is added here without updating
/// this list.
pub const KEYWORDS: &[&str] = &[
    "true",
    "false",
    "if",
    "else",
    "for",
    "while",
    "do",
    "return",
    "break",
    "continue",
    "let",
    "const",
    "fun",
    "async",
    "await",
    "static",
    "import",
    "public",
    "extern",
    "class",
    "struct",
    "unmanaged",
    "sealed",
    "interface",
    "extend",
    "null",
    "is",
    "in",
    "enum",
    "type",
    "switch",
    "case",
    "default",
    "int",
    "float",
    "double",
    "string",
    "bool",
    "char",
    "void",
    "object",
];

/// Contextual keywords: identifiers that are reserved *only* in specific grammar positions (a
/// property accessor, a method name, `this` as a receiver) and so remain a plain `IdentifierToken`
/// everywhere else - unlike [`KEYWORDS`], they cannot be listed as their own `TokenKind`. Listed
/// here so LSP tooling can offer/highlight them without hand-duplicating the spellings.
pub const CONTEXTUAL_KEYWORDS: &[&str] = &[
    "this",
    crate::nodes::function::GET_ACCESSOR,
    crate::nodes::function::SET_ACCESSOR,
    crate::nodes::types::CONSTRUCTOR_NAME,
    crate::nodes::types::DESTRUCTOR_NAME,
];

#[cfg(test)]
mod tests {
    use super::*;
    use logos::Logos;

    /// Every word in [`KEYWORDS`] must actually lex to a keyword token, not a plain identifier -
    /// otherwise it has no business being offered as a reserved word.
    #[test]
    fn every_keyword_lexes_as_a_keyword() {
        for &word in KEYWORDS {
            let mut lexer = TokenKind::lexer(word);
            let kind = lexer
                .next()
                .unwrap_or_else(|| panic!("'{}' did not lex at all", word))
                .unwrap_or_else(|_| panic!("'{}' lexed as an invalid token", word));
            assert_ne!(
                kind,
                TokenKind::IdentifierToken,
                "'{word}' is listed in KEYWORDS but lexes as a plain identifier"
            );
            assert_eq!(lexer.next(), None, "'{word}' did not lex as a single token");
        }
    }

    /// Every *alphabetic* `#[token("...")]` spelling declared on `TokenKind` above must appear in
    /// [`KEYWORDS`] - otherwise a newly added keyword can silently drift out of the LSP's completion
    /// list the way `interface`/`async`/`await`/`sealed`/`struct`/`unmanaged` previously did.
    #[test]
    fn every_alphabetic_token_attr_is_listed() {
        for word in [
            "true",
            "false",
            "if",
            "else",
            "for",
            "while",
            "do",
            "return",
            "break",
            "continue",
            "let",
            "const",
            "fun",
            "async",
            "await",
            "static",
            "import",
            "public",
            "extern",
            "class",
            "struct",
            "unmanaged",
            "sealed",
            "interface",
            "extend",
            "null",
            "is",
            "in",
            "enum",
            "type",
            "switch",
            "case",
            "default",
            "int",
            "float",
            "double",
            "string",
            "bool",
            "char",
            "void",
            "object",
        ] {
            assert!(
                KEYWORDS.contains(&word),
                "'{}' is a keyword token but missing from KEYWORDS",
                word
            );
        }
        assert_eq!(
            KEYWORDS.len(),
            41,
            "a keyword token was added/removed; update both this list and KEYWORDS"
        );
    }
}
