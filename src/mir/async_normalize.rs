//! Await normalization for the async coroutine transform.
//!
//! The async segment splitter (`async_emit::split_async_segments`) only recognizes `await` in three
//! top-level statement shapes: `let x = await e;`, `await e;`, and `return await e;`. To let users
//! write `await` inside sub-expressions (call arguments, operands, array literals, etc.) this pass
//! rewrites each async function's body, hoisting every nested `await` in an *unconditionally
//! evaluated* position into a preceding `let __await_tmp = await ...;` binding and replacing the
//! occurrence with a read of that temporary. Evaluation order is preserved (temporaries are emitted
//! left-to-right, innermost-first).
//!
//! Awaits in *conditionally* evaluated positions — the arms of a ternary, the right operand of
//! `&&`/`||`/`??`, and inside loop/branch bodies or loop conditions — are left untouched; the
//! analyzer (`await_rules`) rejects those, since suspending there would require a full CFG state
//! machine (not yet implemented). This pass therefore only ever produces the top-level shapes the
//! splitter already handles.

use crate::hir::{Binding, HExpr, HExprKind, HFunction, HLocal, HPlace, HStmt, LocalId};
use crate::types::TypeId;

/// Rewrites an async function so every `await` occupies a top-level statement position, hoisting
/// awaits nested in unconditional sub-expressions into fresh `let` temporaries. Non-async functions
/// (and async functions with no nested awaits) are returned effectively unchanged.
pub fn normalize_async_awaits(func: &HFunction) -> HFunction {
    let mut out = func.clone();
    let next_id = out
        .params
        .iter()
        .map(|p| p.local.0)
        .chain(out.locals.iter().map(|l| l.id.0))
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    let mut h = Hoister {
        next_id,
        new_locals: Vec::new(),
    };
    let body = std::mem::take(&mut out.body);
    let mut new_body = Vec::with_capacity(body.len());
    for stmt in body {
        h.hoist_stmt(stmt, &mut new_body);
    }
    out.body = new_body;
    out.locals.extend(h.new_locals);
    out
}

struct Hoister {
    next_id: u32,
    new_locals: Vec<HLocal>,
}

impl Hoister {
    fn fresh(&mut self, ty: TypeId) -> LocalId {
        let id = LocalId(self.next_id);
        self.next_id += 1;
        self.new_locals.push(HLocal {
            id,
            name: format!("__await{}", id.0),
            ty,
        });
        id
    }

    /// Appends `stmt` (with any hoisted `let` temporaries inserted before it) to `out`.
    fn hoist_stmt(&mut self, mut stmt: HStmt, out: &mut Vec<HStmt>) {
        let mut prefix: Vec<HStmt> = Vec::new();
        match &mut stmt {
            // A top-level `let x = await e;` keeps its outer await; only nested awaits in `e` hoist.
            HStmt::Let { value, .. } => {
                if let HExprKind::Await(inner) = &mut value.kind {
                    self.hoist_expr(inner, &mut prefix);
                } else {
                    self.hoist_expr(value, &mut prefix);
                }
            }
            HStmt::Assign { place, value } => {
                self.hoist_place(place, &mut prefix);
                self.hoist_expr(value, &mut prefix);
            }
            HStmt::Expr(e) => self.hoist_expr(e, &mut prefix),
            // A top-level `return await e;` keeps its outer await; only nested awaits in `e` hoist.
            HStmt::Return(Some(e)) => {
                if let HExprKind::Await(inner) = &mut e.kind {
                    self.hoist_expr(inner, &mut prefix);
                } else {
                    self.hoist_expr(e, &mut prefix);
                }
            }
            // A bare `await e;`: `e` is the future expression; hoist any awaits nested within it.
            HStmt::Await(e) => self.hoist_expr(e, &mut prefix),
            // Control-flow statements never contain awaits (the analyzer rejects them), so they pass
            // through untouched.
            _ => {}
        }
        out.extend(prefix);
        out.push(stmt);
    }

    fn hoist_place(&mut self, place: &mut HPlace, prefix: &mut Vec<HStmt>) {
        match place {
            HPlace::Field { obj, .. } => self.hoist_expr(obj, prefix),
            HPlace::Index { array, index } => {
                self.hoist_expr(array, prefix);
                self.hoist_expr(index, prefix);
            }
            HPlace::Local(_) | HPlace::Global(_) => {}
        }
    }

    /// Recursively hoists awaits out of the unconditionally-evaluated sub-expressions of `e`,
    /// rewriting each `await` occurrence into a read of a fresh temporary.
    fn hoist_expr(&mut self, e: &mut HExpr, prefix: &mut Vec<HStmt>) {
        // An `await`: first hoist any awaits nested in the future expression, then lift the whole
        // await into a `let __awaitN = await ...;` bound before the current statement.
        if matches!(e.kind, HExprKind::Await(_)) {
            if let HExprKind::Await(inner) = &mut e.kind {
                self.hoist_expr(inner, prefix);
            }
            let ty = e.ty;
            let id = self.fresh(ty);
            let await_expr =
                std::mem::replace(e, HExpr::new(ty, HExprKind::Var(Binding::Local(id))));
            prefix.push(HStmt::Let {
                local: id,
                ty,
                value: await_expr,
            });
            return;
        }

        match &mut e.kind {
            HExprKind::Binary { op, lhs, rhs } => {
                self.hoist_expr(lhs, prefix);
                // `&&`/`||` evaluate their right operand conditionally; awaits there are rejected by
                // the analyzer, so we must not eagerly hoist them.
                if !op.is_logical() {
                    self.hoist_expr(rhs, prefix);
                }
            }
            HExprKind::Unary { operand, .. } => self.hoist_expr(operand, prefix),
            HExprKind::Call { args, .. } => self.hoist_each(args, prefix),
            HExprKind::MethodCall { receiver, args, .. } => {
                self.hoist_expr(receiver, prefix);
                self.hoist_each(args, prefix);
            }
            HExprKind::IndirectCall { target, args } => {
                self.hoist_expr(target, prefix);
                self.hoist_each(args, prefix);
            }
            HExprKind::InterfaceCall { receiver, args, .. } => {
                self.hoist_expr(receiver, prefix);
                self.hoist_each(args, prefix);
            }
            HExprKind::New { args, .. } | HExprKind::UnionNew { args, .. } => {
                self.hoist_each(args, prefix)
            }
            HExprKind::Field { obj, .. } => self.hoist_expr(obj, prefix),
            HExprKind::Index { array, index } => {
                self.hoist_expr(array, prefix);
                self.hoist_expr(index, prefix);
            }
            HExprKind::ArrayLen(inner)
            | HExprKind::StrLen(inner)
            | HExprKind::HashCode(inner)
            | HExprKind::ToString(inner)
            | HExprKind::Discriminant(inner)
            | HExprKind::Cast(inner) => self.hoist_expr(inner, prefix),
            HExprKind::CharAt(a, b) | HExprKind::Concat(a, b) => {
                self.hoist_expr(a, prefix);
                self.hoist_expr(b, prefix);
            }
            HExprKind::EnumName { value, .. } => self.hoist_expr(value, prefix),
            HExprKind::ArrayNew { len, .. } => self.hoist_expr(len, prefix),
            HExprKind::ArrayLit { elems, .. } => self.hoist_each(elems, prefix),
            HExprKind::UnionField { base, .. } => self.hoist_expr(base, prefix),
            HExprKind::IsType { value, .. } => self.hoist_expr(value, prefix),
            HExprKind::Print { arg, .. } => self.hoist_expr(arg, prefix),
            HExprKind::JsCall { target, method, args, .. } => {
                self.hoist_expr(target, prefix);
                if let Some(m) = method {
                    self.hoist_expr(m, prefix);
                }
                self.hoist_each(args, prefix);
            }
            // Conditionally-evaluated operands: hoist only the unconditional part (the condition /
            // left operand). Awaits in the conditional part are rejected by the analyzer.
            HExprKind::Ternary { cond, .. } => self.hoist_expr(cond, prefix),
            HExprKind::Coalesce { lhs, .. } => self.hoist_expr(lhs, prefix),
            // Leaves (literals, variable reads, enum values, null) contain no sub-expressions.
            _ => {}
        }
    }

    fn hoist_each(&mut self, exprs: &mut [HExpr], prefix: &mut Vec<HStmt>) {
        for e in exprs {
            self.hoist_expr(e, prefix);
        }
    }
}
