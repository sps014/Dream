//! Analysis of expressions, split by concern:
//! - [`dispatch`]: the [`Analyzer::analyze_expression`] match that routes each expression node to its
//!   handler (literals, arrays, indexing, unary, `is`, ternary, `await`, …).
//! - [`member_access`]: struct-field resolution shared by reads/writes and member-read analysis.
//! - [`operators`]: binary-operator typing (coalesce, concat, user `equals`, comparisons).
//! - [`casts`]: `expr as T` validation and the `compare_data_type` assignability check.
//! - [`identifiers`]: identifier resolution (locals, globals, function values) and name→type parsing.

use super::*;

mod casts;
mod dispatch;
mod identifiers;
mod member_access;
mod operators;
