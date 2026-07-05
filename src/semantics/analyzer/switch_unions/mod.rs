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
//!   (`analyze_pattern_switch`) and a general if-chain fallback for guards/nested patterns
//!   (`analyze_pattern_switch_chain`) — plus the subject-resolution/arm-result helpers they share.

use super::*;

mod foreach;
mod lowering;
mod patterns;
mod variant_construction;

/// The HIR shape a switch pattern lowers to (for statement-position `switch` → [`HStmt::Switch`]).
enum HirArmShape {
    /// A `Const` arm (literal pattern).
    Const(crate::hir::HExpr),
    /// A `Variant` arm; `bindings` are the payload local slots in field order.
    Variant {
        def: crate::types::DefId,
        variant: usize,
        bindings: Vec<crate::hir::LocalId>,
    },
    /// A catch-all `_` → the switch `default` block.
    Default,
    /// A catch-all that binds the whole subject to `local` (a bare identifier naming no variant) →
    /// the `default` block, prefixed with `let <name> = <subject>;`.
    DefaultBind {
        local: crate::hir::LocalId,
        ty: crate::types::TypeId,
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
    fn hx_bool(&self, v: bool) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.bool(),
            crate::hir::HExprKind::BoolLit(v),
        )
    }
    fn hx_int(&self, v: i64) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.int(),
            crate::hir::HExprKind::IntLit(v),
        )
    }
    fn hx_local(&self, local: crate::hir::LocalId, ty: crate::types::TypeId) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            ty,
            crate::hir::HExprKind::Var(crate::hir::Binding::Local(local)),
        )
    }
    fn hx_disc(&self, v: crate::hir::HExpr) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.int(),
            crate::hir::HExprKind::Discriminant(Box::new(v)),
        )
    }
    fn hx_bin(
        &self,
        op: crate::hir::BinOp,
        a: crate::hir::HExpr,
        b: crate::hir::HExpr,
    ) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.bool(),
            crate::hir::HExprKind::Binary {
                op,
                lhs: Box::new(a),
                rhs: Box::new(b),
            },
        )
    }
    fn hx_not(&self, a: crate::hir::HExpr) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.bool(),
            crate::hir::HExprKind::Unary {
                op: crate::hir::UnOp::Not,
                operand: Box::new(a),
            },
        )
    }
}
