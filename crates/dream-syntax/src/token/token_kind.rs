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
