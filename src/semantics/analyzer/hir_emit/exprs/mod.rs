//! Expression HIR emitters (`hir_set_*`), split by concern:
//! - [`literals_ops`]: literals, variable/local reads, and the operator/structural expressions
//!   (binary/unary/ternary/coalesce/index/cast/array-literal).
//! - [`calls`]: direct/indirect/generic calls, function values, field/enum reads, constructors,
//!   method/interface/union construction.
//! - [`builtins`]: the compiler-known builtins and runtime shims (`print`, `size`, `to_string`,
//!   `concat`, byte copies, `char_at`, `await`, …).

use super::*;

mod builtins;
mod calls;
mod literals_ops;

impl<'a> Analyzer<'a> {
    /// Collects per-argument HIR (`Vec<Option<HExpr>>`) into a single `Vec<HExpr>`, returning `None`
    /// as soon as any argument dropped out of HIR coverage. Call emitters bail (clearing `last`) when
    /// this returns `None`, so a single un-representable argument suppresses the whole call's HIR.
    fn collect_hir_args(args: Vec<Option<HExpr>>) -> Option<Vec<HExpr>> {
        args.into_iter().collect()
    }
}
