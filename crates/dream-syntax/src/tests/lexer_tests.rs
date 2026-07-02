use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_lex_keywords() {
    let mut lexer =
        Lexer::new("let fun if else for while return break continue import public".to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);
    let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::LetToken,
            TokenKind::FunToken,
            TokenKind::IfToken,
            TokenKind::ElseToken,
            TokenKind::ForToken,
            TokenKind::WhileToken,
            TokenKind::ReturnToken,
            TokenKind::BreakToken,
            TokenKind::ContinueToken,
            TokenKind::ImportToken,
            TokenKind::PublicToken,
            TokenKind::EndOfFileToken,
        ]
    );
}

#[test]
fn test_lex_operators() {
    let mut lexer =
        Lexer::new("+ - * / % == != > < >= <= && || ! = [ ] { } ( ) , . : ;".to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);
    let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::PlusToken,
            TokenKind::MinusToken,
            TokenKind::StarToken,
            TokenKind::SlashToken,
            TokenKind::ModulusToken,
            TokenKind::EqualEqualToken,
            TokenKind::NotEqualToken,
            TokenKind::GreaterThanToken,
            TokenKind::SmallerThanToken,
            TokenKind::GreaterThanEqualToken,
            TokenKind::SmallerThanEqualToken,
            TokenKind::AmpersandAmpersandToken,
            TokenKind::PipePipeToken,
            TokenKind::BangToken,
            TokenKind::EqualToken,
            TokenKind::OpenBracketToken,
            TokenKind::CloseBracketToken,
            TokenKind::CurlyOpenBracketToken,
            TokenKind::CurlyCloseBracketToken,
            TokenKind::OpenParenthesisToken,
            TokenKind::CloseParenthesisToken,
            TokenKind::CommaToken,
            TokenKind::DotToken,
            TokenKind::ColonToken,
            TokenKind::SemicolonToken,
            TokenKind::EndOfFileToken,
        ]
    );
}

#[test]
fn test_lex_literals() {
    let mut lexer = Lexer::new("42 3.14 \"hello\" true false".to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);
    let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::NumberToken,
            TokenKind::NumberToken,
            TokenKind::StringToken,
            TokenKind::BooleanToken,
            TokenKind::BooleanToken,
            TokenKind::EndOfFileToken,
        ]
    );
}

#[test]
fn test_lex_suffixed_number_literals() {
    // Each suffixed numeric literal must lex as a single `NumberToken` carrying the whole text
    // (suffix included); the parser later strips the suffix and assigns the concrete type.
    let mut lexer = Lexer::new("42L 7u 7U 9uL 9UL 9Lu 255b 255B 3.0d 3.0f".to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);
    let texts: Vec<String> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::NumberToken)
        .map(|t| t.text.clone())
        .collect();
    assert_eq!(
        texts,
        vec!["42L", "7u", "7U", "9uL", "9UL", "9Lu", "255b", "255B", "3.0d", "3.0f"]
    );
}

#[test]
fn test_lex_interpolated_string() {
    let mut lexer = Lexer::new("$\"hi {x}\"".to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);
    let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::InterpolatedStringToken,
            TokenKind::EndOfFileToken,
        ]
    );
    assert_eq!(tokens[0].text, "$\"hi {x}\"");
}

#[test]
fn test_lex_bad_token() {
    let mut lexer = Lexer::new("let # = 5;".to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), true);
    assert_eq!(diagnostics.diagnostics.len(), 1);
    assert_eq!(diagnostics.diagnostics[0].message, "unexpected token '#'");

    // The bad token should be skipped
    let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::LetToken,
            TokenKind::EqualToken,
            TokenKind::NumberToken,
            TokenKind::SemicolonToken,
            TokenKind::EndOfFileToken,
        ]
    );
}

#[test]
fn test_lex_trivia() {
    let source = "let x = 1; /* trailing 1 */ // trailing 2\n// leading 1\nlet y = 2;";
    let mut lexer = Lexer::new(source.to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);

    // let x = 1;
    let semi_token = &tokens[4];
    assert_eq!(semi_token.kind, TokenKind::SemicolonToken);

    // Trailing trivia of ';' should contain the two trailing comments
    assert_eq!(semi_token.trailing_trivia.len(), 2);
    assert_eq!(semi_token.trailing_trivia[0].text, "/* trailing 1 */");
    assert_eq!(semi_token.trailing_trivia[1].text, "// trailing 2");

    // let y = 2;
    let next_let = &tokens[5];
    assert_eq!(next_let.kind, TokenKind::LetToken);

    // Leading trivia of the next 'let' should contain the leading comment
    assert_eq!(next_let.leading_trivia.len(), 1);
    assert_eq!(next_let.leading_trivia[0].text, "// leading 1");
}

#[test]
fn test_lex_struct_keyword() {
    let mut lexer = Lexer::new("struct Vec2 { }".to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);
    let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::StructToken,
            TokenKind::IdentifierToken,
            TokenKind::CurlyOpenBracketToken,
            TokenKind::CurlyCloseBracketToken,
            TokenKind::EndOfFileToken,
        ]
    );
}

#[test]
fn test_lex_declaration_comments() {
    let source = "
// Class comment
class MyClass {
    // Field comment
    myField: int;

    // Method comment
    fun myMethod() {}
}
";
    let mut lexer = Lexer::new(source.to_string());
    let mut diagnostics = DiagnosticBag::new(None);
    let tokens = lexer.lex_all(&mut diagnostics);

    assert_eq!(diagnostics.has_errors(), false);

    let class_token = tokens
        .iter()
        .find(|t| t.kind == TokenKind::ClassToken)
        .unwrap();
    assert_eq!(class_token.leading_trivia.len(), 1);
    assert_eq!(class_token.leading_trivia[0].text, "// Class comment");

    let field_token = tokens
        .iter()
        .find(|t| t.kind == TokenKind::IdentifierToken && t.text == "myField")
        .unwrap();
    assert_eq!(field_token.leading_trivia.len(), 1);
    assert_eq!(field_token.leading_trivia[0].text, "// Field comment");

    let fun_token = tokens
        .iter()
        .find(|t| t.kind == TokenKind::FunToken)
        .unwrap();
    assert_eq!(fun_token.leading_trivia.len(), 1);
    assert_eq!(fun_token.leading_trivia[0].text, "// Method comment");
}
