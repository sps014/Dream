pub mod driver;
#[cfg(feature = "native")]
pub mod execution;

// Front-end leaves re-exported for the CLI/LSP facade.
pub use dream_diagnostics as diagnostics;
pub use dream_syntax as syntax;
pub use dream_text as text;
