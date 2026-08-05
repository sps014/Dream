//! The typed, name-resolved High-level IR (HIR).
//!
//! The analyzer lowers the AST to HIR after type-checking, recording everything codegen used to
//! re-derive: every expression carries a [`TypeId`]; every variable reference is a resolved
//! [`Binding`]; every call names a resolved [`Callee`]. Control flow is still structured here —
//! desugaring into a CFG happens in MIR. Monomorphization is an explicit [`MonoInstance`] worklist.

pub mod layout;
pub mod ops;
mod module;
mod nodes;

pub use layout::{scalar_size, FieldLayout, LayoutTable, TypeLayout, UnionLayout, UnionVariant};
pub use ops::{BinOp, UnOp};
pub use module::*;
pub use nodes::*;
