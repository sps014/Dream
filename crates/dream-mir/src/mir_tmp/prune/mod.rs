//! Whole-module tree-shaking: reachability-based function pruning plus the symbol-table shaking that
//! rides on it (dead globals and unreferenced `extern` imports).
//!
//! The analyzer lowers *every* fully-typed function into the module, including the unused
//! standard-prelude helpers (`List`, `Map`, `JsonValue`, …) merged into every program. This pass
//! computes what is reachable from the entry points and drops the rest, so the backend never has to
//! resolve dead code (which may reference runtime pieces the MIR backend has not wired yet) and the
//! emitted module stays small.
//!
//! Split by concern:
//! - [`hir_edges`]: the HIR call-edge walkers ([`hir_body_edges`]) that recover call/type/string
//!   edges from an `async` body's preserved HIR (reused by the emitter for liveness of async bodies
//!   and string/itable shaking).
//! - [`dead_code`]: the MIR reachability core ([`prune_module`]) that drops unreachable functions and
//!   then removes dead globals.

use crate::types::{DefId, TypeId};

mod dead_code;
mod hir_edges;

pub use dead_code::prune_module;
pub(crate) use hir_edges::{hir_body_edges, HirEdges};

/// Identity of a function/instance for the call graph: its def plus the concrete type-args of the
/// monomorphized instance (empty for non-generic functions), matching `MirFunction::{def, instance}`
/// and `Callee::{def, args}`.
type FnKey = (DefId, Vec<TypeId>);
