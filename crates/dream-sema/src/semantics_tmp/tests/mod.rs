//! Analyzer test suite, split by concern to keep each file focused:
//! - [`harness`]: shared compile/run helpers and Dream source stubs.
//! - [`emission_tests`]: HIR->MIR->WAT emission and native execution.
//! - [`analysis_tests`]: type-checking and diagnostics.

mod analysis_tests;
mod emission_tests;
mod harness;
