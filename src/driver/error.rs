//! The top-level, typed error returned by [`crate::driver::compiler::Compiler::compile`]. Each
//! variant names the pipeline phase that failed. User-facing detail for `Syntax`/`Semantic` lives
//! in the diagnostics that were already rendered; `Io` wraps lower-level source/artifact failures.

use std::fmt;

#[derive(Debug)]
pub enum CompileError {
    /// One or more syntax errors were reported during parsing/import resolution.
    Syntax,
    /// One or more semantic errors were reported during analysis.
    Semantic,
    /// An I/O failure during the pipeline (reading sources, writing artifacts).
    Io(std::io::Error),
    /// Code generation hit an internal invariant violation (see `crate::internal_error!`) - a
    /// compiler bug on an otherwise-valid program, not a problem with the user's source. Caught at
    /// the top of [`crate::driver::compiler::Compiler::compile`] so it surfaces as a clean message
    /// instead of an unwinding panic with a raw Rust backtrace.
    Internal(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::Syntax => write!(f, "Syntax errors found during parsing"),
            CompileError::Semantic => write!(f, "Semantic errors found"),
            CompileError::Io(e) => write!(f, "{}", e),
            CompileError::Internal(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<std::io::Error> for CompileError {
    fn from(e: std::io::Error) -> Self {
        CompileError::Io(e)
    }
}
