//! Semantic analysis: the middle of the pipeline between parsing and MIR lowering. The [`analyzer`]
//! walks the parse tree to resolve names, type-check, enforce visibility/flow rules, monomorphize
//! generics, and emit HIR (the backend's input). The sibling tables are the state it builds and
//! queries: [`symbol_table`] (lexical scopes/locals), [`function_table`] (function/method
//! signatures + overloads), [`struct_table`]/[`union_table`] (type layouts and variants), and
//! [`function_control_flow`] (return/flow checks). [`errors`] is the analysis error type.

pub mod analyzer;
pub mod errors;
mod function_control_flow;
pub mod function_table;
pub mod struct_table;
pub mod symbol_table;
pub mod union_table;
