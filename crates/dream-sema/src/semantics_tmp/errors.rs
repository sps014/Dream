use crate::text::text_span::TextSpan;
use std::fmt;

/// A semantic-analysis failure. The analyzer is diagnostics-first: the human-readable message is
/// always pushed into the [`crate::diagnostics::DiagnosticBag`] at the point of failure, and this
/// typed value is returned so `?` can short-circuit the rest of the offending expression. The
/// individual variants let callers (and tests) reason about *what* failed without re-parsing the
/// bag.
#[derive(Debug, Clone)]
pub enum SemanticError {
    /// A specific analysis failure that has already been reported to the diagnostic bag. Carries
    /// the same message/span so it can be inspected directly.
    Reported {
        message: String,
        span: Option<TextSpan>,
    },
    /// Aggregate signal returned by the top-level `analyze` when one or more errors were reported
    /// during analysis (the individual diagnostics live in the `DiagnosticBag`).
    AnalysisFailed,
}

impl SemanticError {
    /// Builds a [`SemanticError::Reported`] from a message and optional span.
    pub fn reported(message: String, span: Option<TextSpan>) -> Self {
        SemanticError::Reported { message, span }
    }
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticError::Reported { message, .. } => write!(f, "{}", message),
            SemanticError::AnalysisFailed => write!(f, "semantic analysis failed"),
        }
    }
}

impl std::error::Error for SemanticError {}

/// Error produced by symbol- and function-table lookups/insertions.
///
/// Previously these tables fabricated `std::io::Error` values for what are
/// really *semantic* failures (e.g. "variable does not exist"). This dedicated
/// type makes the intent explicit while still converting into `std::io::Error`
/// so existing codegen call sites that propagate with `?` keep working and the
/// surfaced message stays identical.
#[derive(Debug, Clone)]
pub struct SymbolError {
    pub message: String,
}

impl SymbolError {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for SymbolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SymbolError {}

impl From<SymbolError> for std::io::Error {
    fn from(err: SymbolError) -> Self {
        std::io::Error::other(err.message)
    }
}
