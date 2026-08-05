//! Lowering from the structured HIR to the CFG-based MIR.
//!
//! All structured control flow is desugared into basic blocks here: `if`/`while`/`for`/`foreach`
//! become block graphs, and the short-circuiting forms (`&&`, `||`, `?:`) materialize their
//! result into a temporary across branches. Every non-trivial expression is reduced to an
//! [`Operand`] (a local read or a constant); intermediate computations are written into fresh
//! temporaries. Reference-counting is left to a dedicated MIR pass ; this stage only
//! produces the data/control skeleton.
//!
//! Split by concern:
//! - [`control_flow`]: `if`/`while`/`do-while`/`for`/`foreach` desugaring into block graphs, and
//!   `break`/`continue` label resolution.
//! - [`switch`]: `switch`/`match` lowering (string content-equality chain, int/enum `br_table`, or
//!   union-variant dispatch with payload binding), dispatched by [`Lowerer::lower_switch`].
//! - [`expr`]: expression lowering to [`Operand`]/[`Rvalue`]/[`Place`], including the
//!   short-circuiting forms (`&&`/`||`/`?:`) and `await`.

use super::build::FunctionBuilder;
use super::{Const, Local, Mir, MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use dream_hir::{Binding, HExpr, HExprKind, HFunction, HParam, HPlace, HStmt, Hir};
use dream_types::{DefId, PrimTy, TyKind, TypeId, TypeInterner};
use std::collections::HashMap;

mod control_flow;
mod expr;
mod switch;

/// Symbol/name of the synthesized module-init function; the backend wires it to `(start ...)`.
pub const INIT_FN_NAME: &str = "__dream_init";

/// True if `a` and `b` are both a bare read of the same local/global binding (e.g. both `this`).
/// Used to recognize the `obj.f = Buffer.realloc<T>(obj.f, n)` self-realloc idiom in
/// [`Lowerer::lower_stmt`] without attempting any deeper structural comparison (a `Field`/`Index`
/// base expression is conservatively never treated as matching).
fn same_local_var(a: &HExpr, b: &HExpr) -> bool {
    matches!(
        (&a.kind, &b.kind),
        (HExprKind::Var(Binding::Local(l1)), HExprKind::Var(Binding::Local(l2))) if l1 == l2
    ) || matches!(
        (&a.kind, &b.kind),
        (HExprKind::Var(Binding::Global(g1)), HExprKind::Var(Binding::Global(g2))) if g1 == g2
    )
}

/// Lowers a whole HIR program to MIR.
pub fn lower_program(hir: &Hir, interner: &TypeInterner) -> Mir {
    let mut functions = Vec::new();
    for f in &hir.functions {
        functions.push(lower_function(f, interner));
    }
    // Synthesize a module-init function from the global initializers, so a `(start ...)` can run
    // them before `main`. Reserves a sentinel `DefId` that no real declaration uses.
    let init_body: Vec<HStmt> = hir
        .globals
        .iter()
        .filter_map(|g| {
            g.init.clone().map(|value| HStmt::Assign {
                place: HPlace::Global(g.id),
                value,
            })
        })
        .collect();
    if !init_body.is_empty() {
        let init_fn = HFunction {
            def: DefId(u32::MAX),
            name: INIT_FN_NAME.to_string(),
            instance: vec![],
            params: Vec::<HParam>::new(),
            ret: interner.void(),
            locals: vec![],
            body: init_body,
            is_async: false,
            file: None,
        };
        functions.push(lower_function(&init_fn, interner));
    }
    let globals = hir
        .globals
        .iter()
        .map(|g| super::MirGlobal {
            id: super::Global(g.id.0),
            ty: g.ty,
        })
        .collect();
    Mir {
        functions,
        globals,
        layouts: hir.layouts.clone(),
        imports: hir.imports.clone(),
        intrinsics: hir.intrinsics.clone(),
        interfaces: hir.interfaces.clone(),
    }
}

/// Lowers a single function.
pub fn lower_function(func: &HFunction, interner: &TypeInterner) -> MirFunction {
    if func.is_async {
        // The pipeline representation of an async function is a stub carrying the HIR body; the poll
        // state machine is lowered from it at emit time (see [`lower_async_poll_body`]), where each
        // `await` becomes a CFG suspend point — so no statement-position normalization is needed.
        return lower_async_stub(func);
    }
    lower_sync_function(func, interner)
}

/// Creates a [`FunctionBuilder`] for `func` (return type, def, source file, async flag) and registers
/// its parameters and declared locals, returning the builder and the HIR-local-id -> MIR-[`Local`]
/// map. Shared by the sync, async-stub, and async-poll lowering entry points, which then differ only
/// in body lowering and terminal handling.
fn init_builder(func: &HFunction, is_async: bool) -> (FunctionBuilder, HashMap<u32, Local>) {
    let mut b = FunctionBuilder::new(func.name.clone(), func.ret);
    b.set_async(is_async);
    b.set_def(func.def, func.instance.clone());
    b.set_file(func.file.clone());
    let mut locals: HashMap<u32, Local> = HashMap::new();
    for p in &func.params {
        let l = if p.is_ref {
            b.new_ref_param(p.ty, Some(p.name.clone()))
        } else {
            b.new_param(p.ty, Some(p.name.clone()))
        };
        locals.insert(p.local.0, l);
    }
    for decl in &func.locals {
        let l = b.new_local(decl.ty, Some(decl.name.clone()));
        locals.insert(decl.id.0, l);
    }
    (b, locals)
}

/// Preserves the HIR body for the async coroutine transform; the poll/constructor are emitted
/// separately (see [`crate::async_emit`]).
fn lower_async_stub(func: &HFunction) -> MirFunction {
    let (mut b, _locals) = init_builder(func, true);
    b.terminate(Terminator::Return(None));
    let mut f = b.finish();
    f.ret = func.ret;
    f.hir_fn = Some(func.clone());
    f
}

fn lower_sync_function(func: &HFunction, interner: &TypeInterner) -> MirFunction {
    let (b, locals) = init_builder(func, func.is_async);

    let mut lo = Lowerer {
        b,
        interner,
        locals,
        loops: Vec::new(),
        locks: Vec::new(),
        async_coroutine: false,
    };
    lo.lower_block(&func.body);

    // Functions that fall off the end implicitly return nothing.
    if !lo.b.is_terminated() {
        lo.b.terminate(Terminator::Return(None));
    }
    lo.b.finish()
}

/// Lowers a complete async function body into a coroutine CFG for the poll state machine: the whole
/// body becomes one block graph in which every `await` is a [`Terminator::Await`] suspend point
/// (see [`Lowerer::lower_await`]), so awaits work in any control-flow position (branches, loops,
/// `switch`, ternary arms). `return` becomes [`Terminator::AsyncComplete`]; falling off the end
/// completes the task with no value. The async backend ([`crate::async_emit`]) turns each
/// `Await`'s `resume` block id into the saved poll state.
pub fn lower_async_poll_body(func: &HFunction, interner: &TypeInterner) -> MirFunction {
    let (b, locals) = init_builder(func, true);
    let mut lo = Lowerer {
        b,
        interner,
        locals,
        loops: Vec::new(),
        locks: Vec::new(),
        async_coroutine: true,
    };
    lo.lower_block(&func.body);
    if !lo.b.is_terminated() {
        lo.b.terminate(Terminator::AsyncComplete(None));
    }
    let mut f = lo.b.finish();
    f.ret = func.ret;
    f
}

struct LoopCtx {
    break_blk: super::BlockId,
    continue_blk: super::BlockId,
    label: Option<String>,
    /// `self.locks.len()` at the point the loop was entered: a `break`/`continue` releases every
    /// lock acquired *inside* the loop (`self.locks[lock_depth..]`, innermost first) but leaves any
    /// lock already held when the loop started untouched.
    lock_depth: usize,
}

struct Lowerer<'a> {
    b: FunctionBuilder,
    interner: &'a TypeInterner,
    locals: HashMap<u32, Local>,
    loops: Vec<LoopCtx>,
    /// Addresses (as MIR locals) of every `lock (obj) { ... }` currently entered, outermost first —
    /// see [`Lowerer::lower_lock`] and the release-on-every-exit-path logic in `lower_break`/
    /// `lower_continue`/`HStmt::Return`.
    locks: Vec<Local>,
    /// When set, this is an async coroutine body: `return` completes the async task (rather than
    /// returning from a WASM function), and each `await` lowers to a [`Terminator::Await`] suspend
    /// point that splits the current block (so awaits work in any control-flow position).
    async_coroutine: bool,
}

impl Lowerer<'_> {
    fn mir_local(&self, hir_local: dream_hir::LocalId) -> Local {
        self.locals[&hir_local.0]
    }

    fn lower_block(&mut self, stmts: &[HStmt]) {
        for s in stmts {
            if self.b.is_terminated() {
                break; // unreachable tail
            }
            self.lower_stmt(s);
        }
    }

    fn lower_stmt(&mut self, stmt: &HStmt) {
        match stmt {
            HStmt::Let { local, value, .. } => {
                let rv = self.lower_rvalue(value);
                let dest = self.mir_local(*local);
                self.b.assign(Place::Local(dest), rv);
            }
            HStmt::Assign { place, value } => {
                // `this.f = Buffer.realloc<T>(this.f, n)` (the `List<T>.grow`/`Pointer<T>.realloc`
                // idiom): lowering `array` through the ordinary `lower_operand` path would
                // materialize the field read into a fresh temp, losing the fact that it reads the
                // very place being overwritten. That matters because `$realloc` already consumes
                // the old block itself (frees it outright if it moved) — the backend's
                // release-old-occupant step for a field/element store
                // (`Emitter::emit_place_store_no_release_old`) only recognizes this and skips the
                // now-double-free-prone release when the `Rvalue::ArrayRealloc` operand is literally
                // `Operand::Copy` of the destination `Place`, so this constructs it directly rather
                // than going through `lower_rvalue`/`lower_place` independently. Local-variable
                // self-realloc (`x = Buffer.realloc<T>(x, n)`) does not need this: `lower_operand`
                // already returns a direct `Place::Local` copy with no temp in that case.
                if let (
                    HPlace::Field {
                        obj,
                        field: dst_field,
                    },
                    HExprKind::ArrayRealloc {
                        elem_ty,
                        array,
                        new_len,
                    },
                ) = (place, &value.kind)
                {
                    if let HExprKind::Field {
                        obj: src_obj,
                        field: src_field,
                    } = &array.kind
                    {
                        if dst_field == src_field && same_local_var(obj, src_obj) {
                            let base = self.operand_into_local(obj);
                            let dest = Place::Field {
                                base,
                                field: *dst_field,
                            };
                            let new_len_op = self.lower_operand(new_len);
                            let rv = Rvalue::ArrayRealloc {
                                elem_ty: *elem_ty,
                                array: Operand::Copy(dest.clone()),
                                new_len: new_len_op,
                            };
                            self.b.assign(dest, rv);
                            return;
                        }
                    }
                }
                let rv = self.lower_rvalue(value);
                let p = self.lower_place(place);
                self.b.assign(p, rv);
            }
            // A bare `await e;` in a coroutine suspends on the future and discards its result.
            HStmt::Await(e) if self.async_coroutine => {
                let fut = self.lower_operand(e);
                let resume = self.b.new_block();
                self.b.terminate(Terminator::Await {
                    future: fut,
                    dest: None,
                    resume,
                });
                self.b.switch_to(resume);
            }
            HStmt::Expr(e) | HStmt::Await(e) => match &e.kind {
                // A bare call keeps its `Call` statement form (return value discarded). This matters
                // for void calls: materializing them into a temp (the fallback below) would emit a
                // `local.set` with nothing on the stack. A call whose discarded result is an owned
                // *reference*, however, must be materialized into a temp so RC insertion releases it at
                // scope exit — otherwise the returned object (and anything it owns) leaks.
                HExprKind::Call { callee, args } if !self.interner.is_reference(e.ty) => {
                    let lowered: Vec<Operand> =
                        args.iter().map(|a| self.lower_operand(a)).collect();
                    self.b.push(Statement::Call {
                        callee: self.lower_callee(callee),
                        args: lowered,
                    });
                }
                HExprKind::MethodCall {
                    receiver,
                    callee,
                    args,
                } if !self.interner.is_reference(e.ty) => {
                    let mut lowered = vec![self.lower_operand(receiver)];
                    lowered.extend(args.iter().map(|a| self.lower_operand(a)));
                    self.b.push(Statement::Call {
                        callee: self.lower_callee(callee),
                        args: lowered,
                    });
                }
                HExprKind::InterfaceCall {
                    receiver,
                    iface_id,
                    method_slot,
                    sig,
                    args,
                } if !self.interner.is_reference(e.ty) => {
                    let recv = self.lower_operand(receiver);
                    let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                    self.b.push(Statement::InterfaceCall {
                        receiver: recv,
                        iface_id: *iface_id,
                        method_slot: *method_slot,
                        sig: *sig,
                        args: lowered,
                    });
                }
                HExprKind::IndirectCall { target, args } if !self.interner.is_reference(e.ty) => {
                    let t = self.lower_operand(target);
                    let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                    self.b.push(Statement::IndirectCall {
                        target: t,
                        args: lowered,
                    });
                }
                // `Buffer.free<T>(arr)` (`@unsafe`) lowers to a dedicated void statement (no result
                // to materialize, unlike the `Rvalue::ArrayRealloc` expression form).
                HExprKind::ForceFree(array) => {
                    let o = self.lower_operand(array);
                    self.b.push(Statement::ForceFree(o));
                }
                // `print`/`println` lower to a dedicated statement the backend maps to `print_*`.
                HExprKind::Print { arg, newline } => {
                    let ty = arg.ty;
                    let o = self.lower_operand(arg);
                    self.b.push(Statement::Print {
                        arg: o,
                        ty,
                        newline: *newline,
                    });
                }
                // Any other expression is evaluated for effect and its value discarded.
                _ => {
                    let _ = self.lower_operand(e);
                }
            },
            HStmt::Return(e) => {
                let op = e.as_ref().map(|e| self.lower_operand(e));
                self.release_all_locks();
                if self.async_coroutine {
                    self.b.terminate(Terminator::AsyncComplete(op));
                } else {
                    self.b.terminate(Terminator::Return(op));
                }
            }
            HStmt::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if(cond, then_branch, else_branch),
            HStmt::While { cond, body, label } => self.lower_while(cond, body, label.as_deref()),
            HStmt::DoWhile { cond, body, label } => {
                self.lower_do_while(cond, body, label.as_deref())
            }
            HStmt::For {
                init,
                cond,
                step,
                body,
                label,
            } => self.lower_for(init, cond, step, body, label.as_deref()),
            HStmt::Foreach {
                elem,
                iterable,
                body,
                label,
            } => self.lower_foreach(*elem, iterable, body, label.as_deref()),
            HStmt::Switch {
                scrutinee,
                arms,
                default,
            } => self.lower_switch(scrutinee, arms, default),
            HStmt::Break(label) => self.lower_break(label.as_deref()),
            HStmt::Continue(label) => self.lower_continue(label.as_deref()),
            HStmt::Lock { target, body } => self.lower_lock(target, body),
            HStmt::DebugLine(line) => self.b.push(Statement::DebugLine(*line)),
            HStmt::SourceLine(line) => self.b.push(Statement::SourceLine(*line)),
        }
    }
}

fn const_int_value(e: &HExpr) -> Option<i64> {
    match &e.kind {
        HExprKind::IntLit(v) | HExprKind::EnumValue(v) => Some(*v),
        HExprKind::CharLit(c) => Some(*c as i64),
        _ => None,
    }
}

/// True if a type lowers to a reference (used by RC insertion and the backend).
pub fn is_reference(interner: &TypeInterner, ty: TypeId) -> bool {
    interner.is_reference(ty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_hir::{Binding, HExpr, HExprKind, HFunction, HStmt, LocalId};
    use crate::Terminator;
    use dream_types::{DefKind, TypeCtx};

    #[test]
    fn lowers_if_into_cfg() {
        let mut ctx = TypeCtx::new();
        let def = ctx.register(DefKind::Function, "f", vec![]);
        let int = ctx.interner.int();
        let boolean = ctx.interner.bool();

        // fun f(x: int): int { if (x) { return 1; } return 0; }
        let func = HFunction {
            def,
            name: "f".into(),
            instance: vec![],
            params: vec![dream_hir::HParam {
                local: LocalId(0),
                name: "x".into(),
                ty: int,
                is_ref: false,
            }],
            ret: int,
            locals: vec![],
            is_async: false,
            file: None,
            body: vec![
                HStmt::If {
                    cond: HExpr::new(boolean, HExprKind::Var(Binding::Local(LocalId(0)))),
                    then_branch: vec![HStmt::Return(Some(HExpr::new(int, HExprKind::IntLit(1))))],
                    else_branch: vec![],
                },
                HStmt::Return(Some(HExpr::new(int, HExprKind::IntLit(0)))),
            ],
        };

        let mir = lower_function(&func, &ctx.interner);
        // entry ends in a two-way branch.
        assert!(matches!(
            mir.blocks[mir.entry.0 as usize].terminator,
            Terminator::If { .. }
        ));
        // at least one block returns.
        assert!(mir
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Return(_))));
    }
}
