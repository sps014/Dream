//! HIR for literals, variable/local reads, and the operator/structural expressions.

use super::*;

impl<'a> Analyzer<'a> {
    /// Records the HIR for a literal expression.
    pub(in crate::semantics::analyzer) fn hir_set_literal(&mut self, lit: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let kind = match lit {
            Type::Integer(t) | Type::Long(t) | Type::UInt(t) | Type::ULong(t) | Type::Byte(t) => {
                t.text.parse::<i64>().ok().map(HExprKind::IntLit)
            }
            Type::Float(t) | Type::Double(t) => t.text.parse::<f64>().ok().map(HExprKind::FloatLit),
            Type::Boolean(t) => Some(HExprKind::BoolLit(t.text == "true")),
            // The parser normalizes a char literal's token text to its decimal code point (e.g.
            // `'A'` → "65"), so recover the `char` from that integer rather than the raw glyph.
            Type::Char(t) => t
                .text
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .map(HExprKind::CharLit),
            Type::String(t) => Some(HExprKind::StringLit(string_lit_value(&t.text))),
            // The parser models the bare `null` literal as `Nullable(Void)` until its type is known.
            Type::Nullable(inner) if matches!(**inner, Type::Void) => Some(HExprKind::Null),
            _ => None,
        };
        let mut ty = self.type_ctx.lower(lit);
        // An `int`-typed literal whose value doesn't fit in `i32` is really a `long`: promote its HIR
        // type so the backend emits `i64.const` instead of an out-of-range `i32.const`. (The parser
        // types decimal integer literals as `int` regardless of magnitude.)
        if let Some(HExprKind::IntLit(v)) = &kind {
            if matches!(lit, Type::Integer(_)) && (*v > i32::MAX as i64 || *v < i32::MIN as i64) {
                ty = self.type_ctx.interner.long();
            }
        }
        self.hir.last = kind.map(|k| HExpr::new(ty, k));
    }

    /// Records the HIR for an identifier read: a local-variable reference if the name resolves to a
    /// slot, otherwise `None` (globals and function values are later slices).
    pub(in crate::semantics::analyzer) fn hir_set_var(&mut self, name: &str) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        if let Some(&(local, ty)) = self.hir.locals.get(name) {
            self.hir.last = Some(HExpr::new(ty, HExprKind::Var(Binding::Local(local))));
        } else if let Some(&(global, ty)) = self.hir.globals.get(name) {
            self.hir.last = Some(HExpr::new(ty, HExprKind::Var(Binding::Global(global))));
        } else {
            self.hir.last = None;
        }
    }

    /// Sets `last` to a read of an already-allocated local (used by the match-expression desugar to
    /// yield the result temporary as the match's value).
    pub(in crate::semantics::analyzer) fn hir_set_local_read(
        &mut self,
        local: LocalId,
        ty: TypeId,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        self.hir.last = Some(HExpr::new(ty, HExprKind::Var(Binding::Local(local))));
    }

    /// Records the HIR for a binary expression from its already-collected operands.
    pub(in crate::semantics::analyzer) fn hir_set_binary(
        &mut self,
        lhs: Option<HExpr>,
        opr: &SyntaxToken,
        rhs: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (token_to_binop(opr.kind), lhs, rhs) {
            (Some(op), Some(lhs), Some(rhs)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for a unary expression. Unary `+` is the identity (passes the operand
    /// through); `-` and `!` map to [`UnOp::Neg`]/[`UnOp::Not`].
    pub(in crate::semantics::analyzer) fn hir_set_unary(
        &mut self,
        opr: &SyntaxToken,
        operand: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let op = match opr.kind {
            TokenKind::PlusToken => {
                self.hir.last = operand;
                return;
            }
            TokenKind::MinusToken => UnOp::Neg,
            TokenKind::BangToken => UnOp::Not,
            _ => {
                self.hir.last = None;
                return;
            }
        };
        self.hir.last = operand.map(|operand| {
            let ty = self.type_ctx.lower(result_ty);
            HExpr::new(
                ty,
                HExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
            )
        });
    }

    /// Records the HIR for a `cond ? then : else_` from its already-collected parts.
    pub(in crate::semantics::analyzer) fn hir_set_ternary(
        &mut self,
        cond: Option<HExpr>,
        then_e: Option<HExpr>,
        else_e: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (cond, then_e, else_e) {
            (Some(cond), Some(then_expr), Some(else_expr)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Ternary {
                        cond: Box::new(cond),
                        then_expr: Box::new(then_expr),
                        else_expr: Box::new(else_expr),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for null-coalescing `lhs ?? rhs`.
    pub(in crate::semantics::analyzer) fn hir_set_coalesce(
        &mut self,
        lhs: Option<HExpr>,
        rhs: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (lhs, rhs) {
            (Some(lhs), Some(rhs)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Coalesce {
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for `array[index]` (read position).
    pub(in crate::semantics::analyzer) fn hir_set_index(
        &mut self,
        array: Option<HExpr>,
        index: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (array, index) {
            (Some(array), Some(index)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Index {
                        array: Box::new(array),
                        index: Box::new(index),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for a cast `expr as T`; `target_ty` is the cast's result type.
    pub(in crate::semantics::analyzer) fn hir_set_cast(
        &mut self,
        inner: Option<HExpr>,
        target_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        self.hir.last = inner.map(|inner| {
            let ty = self.type_ctx.lower(target_ty);
            HExpr::new(ty, HExprKind::Cast(Box::new(inner)))
        });
    }

    /// Records the HIR for a non-empty array literal. `result_ty` is the array type (`T[]`); the
    /// element type is taken from it. Fails if any element was not representable.
    pub(in crate::semantics::analyzer) fn hir_set_array_lit(
        &mut self,
        elems: Vec<Option<HExpr>>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let elem_ty = match result_ty {
            Type::Array(inner) => self.type_ctx.lower(inner),
            _ => {
                self.hir.last = None;
                return;
            }
        };
        let Some(collected) = Self::collect_hir_args(elems) else {
            self.hir.last = None;
            return;
        };
        let ty = self.type_ctx.lower(result_ty);
        self.hir.last = Some(HExpr::new(
            ty,
            HExprKind::ArrayLit {
                elem_ty,
                elems: collected,
            },
        ));
    }
}
