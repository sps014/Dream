// Not every integration-test crate that includes this module exercises every helper.
#![allow(dead_code)]

use dream_lsp::analysis::{collect_diagnostics, DiagnosticOut};
use dream_lsp::index::Index;

pub struct TestHarness {
    pub src: String,
    pub offset: usize,
}

impl TestHarness {
    /// Creates a test harness from a source string with a `|` marker indicating the cursor position.
    pub fn new(marked_src: &str) -> Self {
        let offset = marked_src
            .find('|')
            .expect("Test source must contain a `|` cursor marker");
        let src = marked_src.replace("|", "");
        Self { src, offset }
    }

    /// Builds and returns the symbol Index for the source code.
    pub fn index(&self) -> Index {
        Index::build(None, &self.src)
    }

    /// Runs the diagnostic collector and returns the results.
    pub fn diagnostics(&self) -> Vec<DiagnosticOut> {
        collect_diagnostics(None, &self.src)
    }
}

/// A tiny deterministic xorshift PRNG so fuzz tests are reproducible (no external crates).
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> XorShift {
        // A zero state is degenerate for xorshift, so force a non-zero seed.
        XorShift(seed | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    /// A value in `0..n` (returns 0 when `n == 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            self.next_u32() % n
        }
    }
}

/// A handful of small but valid Dream programs used as seeds for truncation/mutation fuzzing.
pub const VALID_SNIPPETS: &[&str] = &[
    "fun main(): void {\n    let x: int = 1;\n    let y: int = x + 2;\n}\n",
    "const FACTOR: int = 10;\nfun scale(n: int): int {\n    return n * FACTOR;\n}\n",
    "class Point {\n    public x: int;\n    public y: int;\n}\nfun main(): void {\n    let p = Point(1, 2);\n}\n",
    "enum Color { Red, Green, Blue }\nfun main(): void {\n    let c: int = 0;\n}\n",
];

/// Drives every read-only language-service entry point over `src` at a spread of cursor offsets.
/// Used by the fuzz tests to assert that no malformed input can make any of them panic.
pub fn exercise_all(src: &str) {
    let idx = Index::build(None, src);
    let len = src.len();
    let raw = [
        0,
        len / 4,
        len / 2,
        (len * 3) / 4,
        len.saturating_sub(1),
        len,
    ];
    for &probe in &raw {
        // Snap to a char boundary so byte-offset slicing inside the queries is always valid.
        let mut off = probe.min(len);
        while off > 0 && !src.is_char_boundary(off) {
            off -= 1;
        }
        let _ = idx.hover(off);
        let _ = idx.definition(off);
        let _ = idx.references(off, true);
        let _ = idx.completions(None, src, off);
        let _ = idx.signature_help(src, off);
    }
    let _ = idx.document_symbols();
    let _ = collect_diagnostics(None, src);
}
