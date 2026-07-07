//! Lowering from the structured HIR to the CFG-based MIR.
//!
//! All structured control flow is desugared into basic blocks here: `if`/`while`/`for`/`foreach`
//! become block graphs, and the short-circuiting forms (`&&`, `||`, `?:`, `??`) materialize their
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
//!   short-circuiting forms (`&&`/`||`/`?:`/`??`) and `await`.

use super::build::FunctionBuilder;
use super::{Const, Local, Mir, MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use crate::hir::{Binding, HExpr, HExprKind, HFunction, HParam, HPlace, HStmt, Hir};
use crate::types::{DefId, PrimTy, TyKind, TypeId, TypeInterner};
use std::collections::HashMap;

mod control_flow;
mod expr;
mod switch;

/// Symbol/name of the synthesized module-init function; the backend wires it to `(start ...)`.
pub const INIT_FN_NAME: &str = "__dream_init";

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
        let l = b.new_param(p.ty, Some(p.name.clone()));
        locals.insert(p.local.0, l);
    }
    for decl in &func.locals {
        let l = b.new_local(decl.ty, Some(decl.name.clone()));
        locals.insert(decl.id.0, l);
    }
    (b, locals)
}

/// Preserves the HIR body for the async coroutine transform; the poll/constructor are emitted
/// separately (see [`crate::mir::async_emit`]).
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
/// completes the task with no value. The async backend ([`crate::mir::async_emit`]) turns each
/// `Await`'s `resume` block id into the saved poll state.
pub fn lower_async_poll_body(func: &HFunction, interner: &TypeInterner) -> MirFunction {
    let (b, locals) = init_builder(func, true);
    let mut lo = Lowerer {
        b,
        interner,
        locals,
        loops: Vec::new(),
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
}

struct Lowerer<'a> {
    b: FunctionBuilder,
    interner: &'a TypeInterner,
    locals: HashMap<u32, Local>,
    loops: Vec<LoopCtx>,
    /// When set, this is an async coroutine body: `return` completes the async task (rather than
    /// returning from a WASM function), and each `await` lowers to a [`Terminator::Await`] suspend
    /// point that splits the current block (so awaits work in any control-flow position).
    async_coroutine: bool,
}

impl Lowerer<'_> {
    fn mir_local(&self, hir_local: crate::hir::LocalId) -> Local {
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
            HStmt::DebugLine(line) => self.b.push(Statement::DebugLine(*line)),
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
    use crate::hir::{Binding, HExpr, HExprKind, HFunction, HStmt, LocalId};
    use crate::mir::Terminator;
    use crate::types::{DefKind, TypeCtx};

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
            params: vec![crate::hir::HParam {
                local: LocalId(0),
                name: "x".into(),
                ty: int,
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
