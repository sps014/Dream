//! HIR call-edge recovery. An `async` function is lowered to a MIR stub (its real control flow is
//! rebuilt from `hir_fn` during the coroutine transform in `async_emit`), so the block-based
//! reachability walk in [`super::dead_code`] cannot see the calls it makes. These walkers recover
//! those edges (plus live types, string literals, and interface calls) from the preserved HIR body so
//! a callee reachable *only* through an async body is not pruned.

use super::FnKey;
use crate::types::TypeId;

/// The edges (call targets, live types) and string literals discovered in an HIR body.
#[derive(Default)]
pub(crate) struct HirEdges {
    pub callees: Vec<FnKey>,
    pub types: Vec<TypeId>,
    pub strings: Vec<String>,
    /// `(iface_id, method_slot)` of every interface call, so reachability can keep the concrete
    /// implementations dispatched through it.
    pub iface_calls: Vec<(usize, usize)>,
}

pub(crate) fn hir_body_edges(body: &[crate::hir::HStmt], out: &mut HirEdges) {
    for stmt in body {
        hir_stmt_edges(stmt, out);
    }
}

fn hir_stmt_edges(stmt: &crate::hir::HStmt, out: &mut HirEdges) {
    use crate::hir::{HPlace, HStmt};
    match stmt {
        HStmt::Let { value, .. } | HStmt::Expr(value) | HStmt::Await(value) => {
            hir_expr_edges(value, out)
        }
        HStmt::Assign { place, value } => {
            match place {
                HPlace::Field { obj, .. } => hir_expr_edges(obj, out),
                HPlace::Index { array, index } => {
                    hir_expr_edges(array, out);
                    hir_expr_edges(index, out);
                }
                HPlace::Local(_) | HPlace::Global(_) => {}
            }
            hir_expr_edges(value, out);
        }
        HStmt::Return(e) => {
            if let Some(e) = e {
                hir_expr_edges(e, out);
            }
        }
        HStmt::If {
            cond,
            then_branch,
            else_branch,
        } => {
            hir_expr_edges(cond, out);
            hir_body_edges(then_branch, out);
            hir_body_edges(else_branch, out);
        }
        HStmt::While { cond, body, .. } | HStmt::DoWhile { cond, body, .. } => {
            hir_expr_edges(cond, out);
            hir_body_edges(body, out);
        }
        HStmt::For {
            init,
            cond,
            step,
            body,
            ..
        } => {
            hir_stmt_edges(init, out);
            hir_expr_edges(cond, out);
            hir_stmt_edges(step, out);
            hir_body_edges(body, out);
        }
        HStmt::Foreach { iterable, body, .. } => {
            hir_expr_edges(iterable, out);
            hir_body_edges(body, out);
        }
        HStmt::Switch {
            scrutinee,
            arms,
            default,
        } => {
            hir_expr_edges(scrutinee, out);
            for arm in arms {
                if let crate::hir::HPattern::Const(e) = &arm.pattern {
                    hir_expr_edges(e, out);
                }
                hir_body_edges(&arm.body, out);
            }
            hir_body_edges(default, out);
        }
        HStmt::Break(_) | HStmt::Continue(_) | HStmt::DebugLine(_) => {}
    }
}

fn hir_expr_edges(e: &crate::hir::HExpr, out: &mut HirEdges) {
    use crate::hir::HExprKind as K;
    match &e.kind {
        K::Call { callee, args } => {
            out.callees.push((callee.def, callee.instance.clone()));
            for a in args {
                hir_expr_edges(a, out);
            }
        }
        K::MethodCall {
            receiver,
            callee,
            args,
        } => {
            out.callees.push((callee.def, callee.instance.clone()));
            hir_expr_edges(receiver, out);
            for a in args {
                hir_expr_edges(a, out);
            }
        }
        K::IndirectCall { target, args } => {
            hir_expr_edges(target, out);
            for a in args {
                hir_expr_edges(a, out);
            }
        }
        K::InterfaceCall {
            receiver,
            iface_id,
            method_slot,
            args,
            ..
        } => {
            out.iface_calls.push((*iface_id, *method_slot));
            hir_expr_edges(receiver, out);
            for a in args {
                hir_expr_edges(a, out);
            }
        }
        K::New { ctor, args, .. } => {
            if let Some(c) = ctor {
                out.callees.push((*c, vec![]));
            }
            out.types.push(e.ty);
            for a in args {
                hir_expr_edges(a, out);
            }
        }
        K::UnionNew { args, .. } => {
            out.types.push(e.ty);
            for a in args {
                hir_expr_edges(a, out);
            }
        }
        K::Binary { lhs, rhs, .. } | K::Concat(lhs, rhs) | K::Coalesce { lhs, rhs } => {
            hir_expr_edges(lhs, out);
            hir_expr_edges(rhs, out);
        }
        K::CharAt(a, b) | K::Index { array: a, index: b } => {
            hir_expr_edges(a, out);
            hir_expr_edges(b, out);
        }
        K::Unary { operand: x, .. }
        | K::Field { obj: x, .. }
        | K::ArrayLen(x)
        | K::StrLen(x)
        | K::HashCode(x)
        | K::ToString(x)
        | K::EnumName { value: x, .. }
        | K::ArrayNew { len: x, .. }
        | K::ToBytes(x)
        | K::FromBytes(x)
        | K::Cast(x)
        | K::Await(x)
        | K::Discriminant(x)
        | K::UnionField { base: x, .. }
        | K::IsType { value: x, .. }
        | K::Print { arg: x, .. } => hir_expr_edges(x, out),
        K::ArrayLit { elems, .. } => {
            for el in elems {
                hir_expr_edges(el, out);
            }
        }
        K::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            hir_expr_edges(cond, out);
            hir_expr_edges(then_expr, out);
            hir_expr_edges(else_expr, out);
        }
        K::JsCall {
            callee,
            target,
            method,
            args,
        } => {
            out.callees.push((callee.def, callee.instance.clone()));
            hir_expr_edges(target, out);
            if let Some(m) = method {
                hir_expr_edges(m, out);
            }
            for a in args {
                hir_expr_edges(a, out);
            }
        }
        K::StringLit(s) => out.strings.push(s.clone()),
        // Taking a function as a first-class value (e.g. `WebWorker(shout)` or passing `foo` to a
        // `fun(...)` parameter) lowers to `Var(Binding::Func(callee))`. In a *sync* body the MIR
        // walk keeps it alive via `Rvalue::FuncRef`, but an `async` body's reachability comes only
        // from these HIR edges, so record the callee here or it would be pruned and its funcref
        // slot would be missing from the function table.
        K::Var(crate::hir::Binding::Func(callee)) => {
            out.callees.push((callee.def, callee.instance.clone()))
        }
        K::IntLit(_)
        | K::FloatLit(_)
        | K::BoolLit(_)
        | K::CharLit(_)
        | K::Null
        | K::Var(_)
        | K::EnumValue(_) => {}
    }
}
