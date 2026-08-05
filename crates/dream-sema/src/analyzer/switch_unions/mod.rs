//! Analysis of discriminated-union construction (`Enum.Variant(args)` / unit `Enum.Variant`) and
//! of pattern-matching `switch` expressions/statements: pattern typing, binding scopes, guards,
//! arm-type unification, exhaustiveness, and unreachable-arm detection.
//!
//! Split by concern:
//! - [`variant_construction`]: type-checks `Enum.Variant(args)` construction (concrete + generic).
//! - [`patterns`]: pattern classification/compilation shared by both switch-lowering paths in
//!   [`lowering`] (`HirArmShape`, `compile_pattern`, `hir_switch_pattern`, `pattern_is_nested`).
//! - [`foreach`]: `for (x in iterable)` desugaring via the enumerator protocol (`iterator()`/`next()`).
//! - [`lowering`]: the two pattern-`switch` lowering paths — a `Switch`/br_table fast path
//!   (`analyze_pattern_switch`, including or-patterns and small literal ranges expanded to multi-key
//!   arms) and a general if-chain fallback for guards/nested patterns
//!   (`analyze_pattern_switch_chain`) — plus the subject-resolution/arm-result helpers they share.

use super::*;

mod exhaustiveness;
mod foreach;
mod lowering;
mod patterns;
mod try_propagation;
mod variant_construction;

/// The HIR shape a switch pattern lowers to (for statement-position `switch` → [`HStmt::Switch`]).
enum HirArmShape {
    /// A `Const` arm (literal pattern).
    Const(dream_hir::HExpr),
    /// A `Variant` arm; `bindings` are the payload local slots in field order.
    Variant {
        def: dream_types::DefId,
        variant: usize,
        bindings: Vec<dream_hir::LocalId>,
    },
    /// A catch-all `_` → the switch `default` block.
    Default,
    /// A catch-all that binds the whole subject to `local` (a bare identifier naming no variant) →
    /// the `default` block, prefixed with `let <name> = <subject>;`.
    DefaultBind {
        local: dream_hir::LocalId,
        ty: dream_types::TypeId,
    },
    /// Not representable in HIR's `Switch` (nested sub-pattern, bad literal).
    Unsupported,
}

/// What checking a single pattern told us, used to drive exhaustiveness and unreachable-arm
/// analysis.
pub(super) struct PatternInfo {
    /// True when the pattern matches every value of its type (a bare binding or `_`). Drives
    /// unreachable-arm detection; full (possibly nested) coverage is computed separately in
    /// [`Analyzer::check_exhaustiveness`] from the arm patterns.
    pub(super) irrefutable: bool,
}

impl<'a> Analyzer<'a> {
    // -- small typed-HExpr builders shared by the pattern-compiler and both switch-lowering paths --
    fn hx_bool(&self, v: bool) -> dream_hir::HExpr {
        dream_hir::HExpr::new(
            self.type_ctx.interner.bool(),
            dream_hir::HExprKind::BoolLit(v),
        )
    }
    fn hx_int(&self, v: i64) -> dream_hir::HExpr {
        dream_hir::HExpr::new(
            self.type_ctx.interner.int(),
            dream_hir::HExprKind::IntLit(v),
        )
    }
    fn hx_local(&self, local: dream_hir::LocalId, ty: dream_types::TypeId) -> dream_hir::HExpr {
        dream_hir::HExpr::new(
            ty,
            dream_hir::HExprKind::Var(dream_hir::Binding::Local(local)),
        )
    }
    fn hx_disc(&self, v: dream_hir::HExpr) -> dream_hir::HExpr {
        dream_hir::HExpr::new(
            self.type_ctx.interner.int(),
            dream_hir::HExprKind::Discriminant(Box::new(v)),
        )
    }
    fn hx_bin(
        &self,
        op: dream_hir::BinOp,
        a: dream_hir::HExpr,
        b: dream_hir::HExpr,
    ) -> dream_hir::HExpr {
        dream_hir::HExpr::new(
            self.type_ctx.interner.bool(),
            dream_hir::HExprKind::Binary {
                op,
                lhs: Box::new(a),
                rhs: Box::new(b),
            },
        )
    }
    fn hx_not(&self, a: dream_hir::HExpr) -> dream_hir::HExpr {
        dream_hir::HExpr::new(
            self.type_ctx.interner.bool(),
            dream_hir::HExprKind::Unary {
                op: dream_hir::UnOp::Not,
                operand: Box::new(a),
            },
        )
    }
}
