//! Robustness tests: the language service must never panic, no matter how malformed the input.
//! These complement the parser-level fuzz tests in the compiler crate by driving the full LSP
//! query surface (hover/definition/references/completion/signature-help/symbols/diagnostics).

mod common;

use common::{exercise_all, XorShift, VALID_SNIPPETS};

#[test]
fn builds_index_for_stdlib_without_panic() {
    let path = "../../src/stdlib/system/core/math.dream";
    let src = std::fs::read_to_string(path).unwrap();
    let idx = dream_lsp::index::Index::build(Some(path), &src);
    assert!(
        !idx.decls.is_empty(),
        "expected the stdlib math module to produce declarations"
    );
}

#[test]
fn random_token_soup_never_panics() {
    // A grab-bag of bytes that look vaguely like Dream source, so the lexer/parser actually do
    // work rather than immediately bailing.
    const ALPHABET: &[u8] = b"abcdefn (){}=;:,.+-*/<>\"[]01_\n";
    let mut rng = XorShift::new(0xDEAD_BEEF);
    for _ in 0..150 {
        let len = rng.below(200) as usize;
        let s: String = (0..len)
            .map(|_| ALPHABET[rng.below(ALPHABET.len() as u32) as usize] as char)
            .collect();
        exercise_all(&s);
    }
}

#[test]
fn truncated_valid_programs_never_panic() {
    // Prefixes of a valid program simulate a half-typed document (every other byte keeps the test
    // fast while still hitting truncations in the middle of every token).
    for snippet in VALID_SNIPPETS {
        for cut in (0..=snippet.len()).step_by(2) {
            if snippet.is_char_boundary(cut) {
                exercise_all(&snippet[..cut]);
            }
        }
    }
}

#[test]
fn byte_mutations_never_panic() {
    const POISON: &[u8] = b" {}();=.+\"\n[]<>";
    let mut rng = XorShift::new(0x1234_5678);
    for snippet in VALID_SNIPPETS {
        let mut bytes = snippet.as_bytes().to_vec();
        for _ in 0..80 {
            if bytes.is_empty() {
                break;
            }
            let idx = rng.below(bytes.len() as u32) as usize;
            bytes[idx] = POISON[rng.below(POISON.len() as u32) as usize];
            // Mutations stay within ASCII, so this is always valid UTF-8.
            let mutated = std::str::from_utf8(&bytes).unwrap();
            exercise_all(mutated);
        }
    }
}
