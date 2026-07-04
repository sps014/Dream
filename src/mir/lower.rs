//! Lowering from the structured HIR to the CFG-based MIR.
//!
//! All structured control flow is desugared into basic blocks here: `if`/`while`/`for`/`foreach`
//! become block graphs, and the short-circuiting forms (`&&`, `||`, `?:`, `??`) materialize their
//! result into a temporary across branches. Every non-trivial expression is reduced to an
//! [`Operand`] (a local read or a constant); intermediate computations are written into fresh
//! temporaries. Reference-counting is left to a dedicated MIR pass ; this stage only
//! produces the data/control skeleton.

use super::build::FunctionBuilder;
use super::{Const, Local, Mir, MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use crate::hir::{Binding, HExpr, HExprKind, HFunction, HParam, HPlace, HStmt, Hir};
use crate::types::{DefId, PrimTy, TyKind, TypeId, TypeInterner};
use std::collections::HashMap;

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
        return lower_async_stub(func, interner);
    }
    lower_sync_function(func, interner)
}

/// Preserves the HIR body for the async coroutine transform; the poll/constructor are emitted
/// separately (see [`crate::mir::async_emit`]).
fn lower_async_stub(func: &HFunction, interner: &TypeInterner) -> MirFunction {
    let mut b = FunctionBuilder::new(func.name.clone(), interner.int());
    b.set_async(true);
    b.set_def(func.def, func.instance.clone());
    for p in &func.params {
        b.new_param(p.ty, Some(p.name.clone()));
    }
    for decl in &func.locals {
        b.new_local(decl.ty, Some(decl.name.clone()));
    }
    b.terminate(Terminator::Return(None));
    let mut f = b.finish();
    f.ret = func.ret;
    f.hir_fn = Some(func.clone());
    f
}

fn lower_sync_function(func: &HFunction, interner: &TypeInterner) -> MirFunction {
    let mut b = FunctionBuilder::new(func.name.clone(), func.ret);
    b.set_async(func.is_async);
    b.set_def(func.def, func.instance.clone());

    let mut locals: HashMap<u32, Local> = HashMap::new();
    for p in &func.params {
        let l = b.new_param(p.ty, Some(p.name.clone()));
        locals.insert(p.local.0, l);
    }
    for decl in &func.locals {
        let l = b.new_local(decl.ty, Some(decl.name.clone()));
        locals.insert(decl.id.0, l);
    }

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
    let mut b = FunctionBuilder::new(func.name.clone(), func.ret);
    b.set_async(true);
    b.set_def(func.def, func.instance.clone());
    let mut locals: HashMap<u32, Local> = HashMap::new();
    for p in &func.params {
        let l = b.new_param(p.ty, Some(p.name.clone()));
        locals.insert(p.local.0, l);
    }
    for decl in &func.locals {
        let l = b.new_local(decl.ty, Some(decl.name.clone()));
        locals.insert(decl.id.0, l);
    }
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
        }
    }

    fn lower_if(&mut self, cond: &HExpr, then_b: &[HStmt], else_b: &[HStmt]) {
        let c = self.lower_operand(cond);
        let then_blk = self.b.new_block();
        let else_blk = self.b.new_block();
        let join = self.b.new_block();
        self.b.terminate(Terminator::If {
            cond: c,
            then_blk,
            else_blk,
        });

        self.b.switch_to(then_blk);
        self.lower_block(then_b);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }

        self.b.switch_to(else_blk);
        self.lower_block(else_b);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }

        self.b.switch_to(join);
    }

    fn lower_while(&mut self, cond: &HExpr, body: &[HStmt], label: Option<&str>) {
        let cond_blk = self.b.new_block();
        let body_blk = self.b.new_block();
        let after_blk = self.b.new_block();
        self.b.terminate(Terminator::Goto(cond_blk));

        self.b.switch_to(cond_blk);
        let c = self.lower_operand(cond);
        self.b.terminate(Terminator::If {
            cond: c,
            then_blk: body_blk,
            else_blk: after_blk,
        });

        self.loops.push(LoopCtx {
            break_blk: after_blk,
            continue_blk: cond_blk,
            label: label.map(str::to_string),
        });
        self.b.switch_to(body_blk);
        self.lower_block(body);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(cond_blk));
        }
        self.loops.pop();

        self.b.switch_to(after_blk);
    }

    /// `do { body } while (cond)`: the body block runs unconditionally first, then the condition is
    /// tested to loop back (`continue` jumps to the condition test, `break` exits).
    fn lower_do_while(&mut self, cond: &HExpr, body: &[HStmt], label: Option<&str>) {
        let body_blk = self.b.new_block();
        let cond_blk = self.b.new_block();
        let after_blk = self.b.new_block();
        self.b.terminate(Terminator::Goto(body_blk));

        self.loops.push(LoopCtx {
            break_blk: after_blk,
            continue_blk: cond_blk,
            label: label.map(str::to_string),
        });
        self.b.switch_to(body_blk);
        self.lower_block(body);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(cond_blk));
        }
        self.loops.pop();

        self.b.switch_to(cond_blk);
        let c = self.lower_operand(cond);
        self.b.terminate(Terminator::If {
            cond: c,
            then_blk: body_blk,
            else_blk: after_blk,
        });

        self.b.switch_to(after_blk);
    }

    fn lower_for(
        &mut self,
        init: &HStmt,
        cond: &HExpr,
        step: &HStmt,
        body: &[HStmt],
        label: Option<&str>,
    ) {
        self.lower_stmt(init);
        let cond_blk = self.b.new_block();
        let body_blk = self.b.new_block();
        let step_blk = self.b.new_block();
        let after_blk = self.b.new_block();
        self.b.terminate(Terminator::Goto(cond_blk));

        self.b.switch_to(cond_blk);
        let c = self.lower_operand(cond);
        self.b.terminate(Terminator::If {
            cond: c,
            then_blk: body_blk,
            else_blk: after_blk,
        });

        self.loops.push(LoopCtx {
            break_blk: after_blk,
            continue_blk: step_blk,
            label: label.map(str::to_string),
        });
        self.b.switch_to(body_blk);
        self.lower_block(body);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(step_blk));
        }
        self.loops.pop();

        self.b.switch_to(step_blk);
        self.lower_stmt(step);
        self.b.terminate(Terminator::Goto(cond_blk));

        self.b.switch_to(after_blk);
    }

    fn lower_foreach(
        &mut self,
        elem: crate::hir::LocalId,
        iterable: &HExpr,
        body: &[HStmt],
        label: Option<&str>,
    ) {
        let int = self.interner.int();
        let arr = self.lower_operand(iterable);
        let arr_local = self.b.new_temp(iterable.ty);
        self.b.assign(Place::Local(arr_local), Rvalue::Use(arr));

        let idx = self.b.new_temp(int);
        self.b.assign(
            Place::Local(idx),
            Rvalue::Use(Operand::Const(Const::Int(0))),
        );
        let len = self.b.new_temp(int);
        self.b.assign(
            Place::Local(len),
            Rvalue::ArrayLen(Operand::Copy(Place::Local(arr_local))),
        );

        let cond_blk = self.b.new_block();
        let body_blk = self.b.new_block();
        let step_blk = self.b.new_block();
        let after_blk = self.b.new_block();
        self.b.terminate(Terminator::Goto(cond_blk));

        self.b.switch_to(cond_blk);
        let cmp = self.b.new_temp(self.interner.bool());
        self.b.assign(
            Place::Local(cmp),
            Rvalue::Binary(
                super::BinOp::Lt,
                Operand::Copy(Place::Local(idx)),
                Operand::Copy(Place::Local(len)),
            ),
        );
        self.b.terminate(Terminator::If {
            cond: Operand::Copy(Place::Local(cmp)),
            then_blk: body_blk,
            else_blk: after_blk,
        });

        self.loops.push(LoopCtx {
            break_blk: after_blk,
            continue_blk: step_blk,
            label: label.map(str::to_string),
        });
        self.b.switch_to(body_blk);
        let elem_local = self.mir_local(elem);
        self.b.assign(
            Place::Local(elem_local),
            Rvalue::Use(Operand::Copy(Place::Index {
                base: arr_local,
                index: Box::new(Operand::Copy(Place::Local(idx))),
            })),
        );
        self.lower_block(body);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(step_blk));
        }
        self.loops.pop();

        self.b.switch_to(step_blk);
        self.b.assign(
            Place::Local(idx),
            Rvalue::Binary(
                super::BinOp::Add,
                Operand::Copy(Place::Local(idx)),
                Operand::Const(Const::Int(1)),
            ),
        );
        self.b.terminate(Terminator::Goto(cond_blk));

        self.b.switch_to(after_blk);
    }

    fn lower_switch(&mut self, scrutinee: &HExpr, arms: &[crate::hir::HArm], default: &[HStmt]) {
        // A union `match` dispatches on the value's discriminant and binds each arm's payload; a
        // string `switch` needs content equality (no `br_table`); an int/enum/bool `switch`
        // dispatches on the scrutinee value itself via a `br_table`.
        if arms
            .iter()
            .any(|a| matches!(a.pattern, crate::hir::HPattern::Variant { .. }))
        {
            self.lower_variant_switch(scrutinee, arms, default);
        } else if matches!(
            self.interner
                .kind(self.interner.strip_nullable(scrutinee.ty)),
            TyKind::Prim(PrimTy::String)
        ) {
            self.lower_string_switch(scrutinee, arms, default);
        } else {
            self.lower_const_switch(scrutinee, arms, default);
        }
    }

    /// Lowers a `switch` on a string: a linear chain of `subject == label` content comparisons (the
    /// backend routes string `==` through `$string_eq`), each branching to its arm or the next check.
    fn lower_string_switch(
        &mut self,
        scrutinee: &HExpr,
        arms: &[crate::hir::HArm],
        default: &[HStmt],
    ) {
        let subject = self.lower_operand(scrutinee);
        let join = self.b.new_block();
        for arm in arms {
            let crate::hir::HPattern::Const(label) = &arm.pattern else {
                continue;
            };
            let label_op = self.lower_operand(label);
            let cond = self.b.new_temp(self.interner.bool());
            self.b.assign(
                Place::Local(cond),
                Rvalue::Binary(super::BinOp::Eq, subject.clone(), label_op),
            );
            let body_blk = self.b.new_block();
            let next_blk = self.b.new_block();
            self.b.terminate(Terminator::If {
                cond: Operand::Copy(Place::Local(cond)),
                then_blk: body_blk,
                else_blk: next_blk,
            });
            self.b.switch_to(body_blk);
            self.lower_block(&arm.body);
            if !self.b.is_terminated() {
                self.b.terminate(Terminator::Goto(join));
            }
            self.b.switch_to(next_blk);
        }
        // Chain fell through every case: run the default, then join.
        self.lower_block(default);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }
        self.b.switch_to(join);
    }

    /// Lowers a `switch`/`match` whose arms are const/enum-valued: a `br_table` over the scrutinee.
    fn lower_const_switch(
        &mut self,
        scrutinee: &HExpr,
        arms: &[crate::hir::HArm],
        default: &[HStmt],
    ) {
        let value = self.lower_operand(scrutinee);
        let default_blk = self.b.new_block();
        let join = self.b.new_block();
        let mut targets: Vec<(i64, super::BlockId)> = Vec::new();

        for arm in arms {
            let blk = self.b.new_block();
            if let crate::hir::HPattern::Const(c) = &arm.pattern {
                if let Some(v) = const_int_value(c) {
                    targets.push((v, blk));
                }
            }
            let saved = self.b.current();
            self.b.switch_to(blk);
            self.lower_block(&arm.body);
            if !self.b.is_terminated() {
                self.b.terminate(Terminator::Goto(join));
            }
            self.b.switch_to(saved);
        }

        self.b.terminate(Terminator::Switch {
            value,
            targets,
            default: default_blk,
        });

        self.b.switch_to(default_blk);
        self.lower_block(default);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }

        self.b.switch_to(join);
    }

    /// Lowers a `match` on a discriminated union: read the value's discriminant, `br_table` on it,
    /// and in each variant arm bind the payload fields to their pattern locals before the body runs.
    fn lower_variant_switch(
        &mut self,
        scrutinee: &HExpr,
        arms: &[crate::hir::HArm],
        default: &[HStmt],
    ) {
        let union_ty = scrutinee.ty;
        let ptr = self.lower_operand(scrutinee);
        let disc = self.b.new_temp(self.interner.int());
        self.b
            .assign(Place::Local(disc), Rvalue::Discriminant(ptr.clone()));

        let default_blk = self.b.new_block();
        let join = self.b.new_block();
        let mut targets: Vec<(i64, super::BlockId)> = Vec::new();

        for arm in arms {
            let crate::hir::HPattern::Variant {
                variant, bindings, ..
            } = &arm.pattern
            else {
                continue;
            };
            let blk = self.b.new_block();
            targets.push((*variant as i64, blk));
            let saved = self.b.current();
            self.b.switch_to(blk);
            // Bind the active variant's payload fields; each is a borrow of the union's field (no
            // retain), so releasing the union later frees them exactly once.
            for (i, &binding) in bindings.iter().enumerate() {
                let local = self.mir_local(binding);
                self.b.assign(
                    Place::Local(local),
                    Rvalue::UnionField {
                        base: ptr.clone(),
                        ty: union_ty,
                        variant: *variant,
                        field: i,
                    },
                );
            }
            self.lower_block(&arm.body);
            if !self.b.is_terminated() {
                self.b.terminate(Terminator::Goto(join));
            }
            self.b.switch_to(saved);
        }

        self.b.terminate(Terminator::Switch {
            value: Operand::Copy(Place::Local(disc)),
            targets,
            default: default_blk,
        });

        self.b.switch_to(default_blk);
        self.lower_block(default);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }

        self.b.switch_to(join);
    }

    fn lower_break(&mut self, label: Option<&str>) {
        if let Some(target) = self.loop_target(label, true) {
            self.b.terminate(Terminator::Goto(target));
        }
    }

    fn lower_continue(&mut self, label: Option<&str>) {
        if let Some(target) = self.loop_target(label, false) {
            self.b.terminate(Terminator::Goto(target));
        }
    }

    fn loop_target(&self, label: Option<&str>, is_break: bool) -> Option<super::BlockId> {
        let ctx = match label {
            Some(l) => self
                .loops
                .iter()
                .rev()
                .find(|c| c.label.as_deref() == Some(l)),
            None => self.loops.last(),
        }?;
        Some(if is_break {
            ctx.break_blk
        } else {
            ctx.continue_blk
        })
    }

    /// Selects the integer constant width from the literal's static type: `long`/`ulong` lower to a
    /// 64-bit [`Const::Long`], everything else (`int`/`uint`/`byte`) to a 32-bit [`Const::Int`].
    fn int_const(&self, ty: TypeId, v: i64) -> Const {
        match self.interner.kind(self.interner.strip_nullable(ty)) {
            TyKind::Prim(PrimTy::Long | PrimTy::ULong) => Const::Long(v),
            _ => Const::Int(v),
        }
    }

    /// Selects the float constant width from the literal's static type: `float` lowers to a 32-bit
    /// [`Const::F32`], `double` (and anything else) to a 64-bit [`Const::Float`].
    fn float_const(&self, ty: TypeId, v: f64) -> Const {
        match self.interner.kind(self.interner.strip_nullable(ty)) {
            TyKind::Prim(PrimTy::Float) => Const::F32(v as f32),
            _ => Const::Float(v),
        }
    }

    /// Lowers an expression to an operand, materializing computation into a fresh temporary.
    fn lower_operand(&mut self, e: &HExpr) -> Operand {
        match &e.kind {
            HExprKind::IntLit(v) => Operand::Const(self.int_const(e.ty, *v)),
            HExprKind::FloatLit(v) => Operand::Const(self.float_const(e.ty, *v)),
            HExprKind::BoolLit(v) => Operand::Const(Const::Bool(*v)),
            HExprKind::CharLit(v) => Operand::Const(Const::Char(*v)),
            HExprKind::StringLit(s) => Operand::Const(Const::Str(s.clone())),
            HExprKind::Null => Operand::Const(Const::Null),
            HExprKind::EnumValue(v) => Operand::Const(Const::Int(*v)),
            HExprKind::Var(Binding::Local(l)) => Operand::Copy(Place::Local(self.mir_local(*l))),
            HExprKind::Var(Binding::Global(g)) => Operand::Copy(Place::Global(super::Global(g.0))),
            HExprKind::Binary { op, .. } if op.is_logical() => self.lower_short_circuit(e),
            HExprKind::Ternary { .. } => self.lower_ternary(e),
            HExprKind::Coalesce { .. } => self.lower_coalesce(e),
            // In a coroutine, `await` is a suspend point that splits the current block; elsewhere the
            // outer await is a no-op wrapper (the value is the inner future expression).
            HExprKind::Await(inner) => {
                if self.async_coroutine {
                    self.lower_await(e)
                } else {
                    self.lower_operand(inner)
                }
            }
            _ => {
                let rv = self.lower_rvalue(e);
                let temp = self.b.new_temp(e.ty);
                self.b.assign(Place::Local(temp), rv);
                Operand::Copy(Place::Local(temp))
            }
        }
    }

    /// Lowers an expression into an rvalue (the form usable on an assignment RHS).
    fn lower_rvalue(&mut self, e: &HExpr) -> Rvalue {
        match &e.kind {
            HExprKind::Binary { op, lhs, rhs } if !op.is_logical() => {
                let l = self.lower_operand(lhs);
                let r = self.lower_operand(rhs);
                Rvalue::Binary(*op, l, r)
            }
            HExprKind::Unary { op, operand } => {
                let o = self.lower_operand(operand);
                Rvalue::Unary(*op, o)
            }
            HExprKind::Call { callee, args } => {
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::Call {
                    callee: self.lower_callee(callee),
                    args: lowered,
                }
            }
            HExprKind::MethodCall {
                receiver,
                callee,
                args,
            } => {
                let mut lowered = vec![self.lower_operand(receiver)];
                lowered.extend(args.iter().map(|a| self.lower_operand(a)));
                Rvalue::Call {
                    callee: self.lower_callee(callee),
                    args: lowered,
                }
            }
            HExprKind::IndirectCall { target, args } => {
                let t = self.lower_operand(target);
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::IndirectCall {
                    target: t,
                    args: lowered,
                }
            }
            HExprKind::InterfaceCall {
                receiver,
                iface_id,
                method_slot,
                sig,
                args,
            } => {
                let recv = self.lower_operand(receiver);
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::InterfaceCall {
                    receiver: recv,
                    iface_id: *iface_id,
                    method_slot: *method_slot,
                    sig: *sig,
                    args: lowered,
                    ret: e.ty,
                }
            }
            // A function name used as a value becomes its function-table index.
            HExprKind::Var(Binding::Func(callee)) => Rvalue::FuncRef(self.lower_callee(callee)),
            HExprKind::New {
                def, ctor, args, ..
            } => {
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::New {
                    def: *def,
                    ty: e.ty,
                    ctor: *ctor,
                    args: lowered,
                }
            }
            HExprKind::UnionNew { def, variant, args } => {
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::UnionNew {
                    def: *def,
                    ty: e.ty,
                    variant: *variant,
                    args: lowered,
                }
            }
            HExprKind::Field { obj, field } => {
                let base = self.operand_into_local(obj);
                Rvalue::Use(Operand::Copy(Place::Field {
                    base,
                    field: *field,
                }))
            }
            HExprKind::Index { array, index } => {
                let base = self.operand_into_local(array);
                let idx = self.lower_operand(index);
                Rvalue::Use(Operand::Copy(Place::Index {
                    base,
                    index: Box::new(idx),
                }))
            }
            HExprKind::Discriminant(v) => Rvalue::Discriminant(self.lower_operand(v)),
            HExprKind::IsType { value, target } => {
                Rvalue::IsType(self.lower_operand(value), *target)
            }
            HExprKind::UnionField {
                base,
                union_ty,
                variant,
                field,
            } => Rvalue::UnionField {
                base: self.lower_operand(base),
                ty: *union_ty,
                variant: *variant,
                field: *field,
            },
            HExprKind::ArrayLen(a) => Rvalue::ArrayLen(self.lower_operand(a)),
            HExprKind::StrLen(a) => Rvalue::StrLen(self.lower_operand(a)),
            HExprKind::CharAt(s, i) => Rvalue::CharAt(self.lower_operand(s), self.lower_operand(i)),
            HExprKind::ArrayNew { elem_ty, len } => Rvalue::ArrayNew {
                elem_ty: *elem_ty,
                len: self.lower_operand(len),
            },
            HExprKind::HashCode(e) => Rvalue::HashCode(self.lower_operand(e)),
            HExprKind::ToString(e) => Rvalue::ToString(self.lower_operand(e)),
            HExprKind::Concat(a, b) => Rvalue::Concat(self.lower_operand(a), self.lower_operand(b)),
            HExprKind::EnumName { value, arms } => Rvalue::EnumName {
                value: self.lower_operand(value),
                arms: arms.clone(),
            },
            HExprKind::ArrayLit { elem_ty, elems } => {
                let lowered = elems.iter().map(|e| self.lower_operand(e)).collect();
                Rvalue::ArrayLit {
                    elem_ty: *elem_ty,
                    elems: lowered,
                }
            }
            HExprKind::JsCall {
                callee,
                target,
                method,
                args,
            } => {
                let target = self.lower_operand(target);
                let method = method.as_ref().map(|m| self.lower_operand(m));
                let args = args.iter().map(|a| (self.lower_operand(a), a.ty)).collect();
                Rvalue::JsCall {
                    callee: self.lower_callee(callee),
                    target,
                    method,
                    args,
                }
            }
            HExprKind::Cast(inner) => {
                let from = inner.ty;
                Rvalue::Cast(self.lower_operand(inner), from, e.ty)
            }
            // `await` as an rvalue routes through `lower_operand`, which suspends in a coroutine
            // (materializing the awaited result into a temp) or unwraps to the inner future otherwise.
            HExprKind::Await(_) => Rvalue::Use(self.lower_operand(e)),
            // Already-operand-shaped or short-circuiting forms: go through `lower_operand`.
            _ => Rvalue::Use(self.lower_operand(e)),
        }
    }

    fn operand_into_local(&mut self, e: &HExpr) -> Local {
        match self.lower_operand(e) {
            Operand::Copy(Place::Local(l)) => l,
            other => {
                let t = self.b.new_temp(e.ty);
                self.b.assign(Place::Local(t), Rvalue::Use(other));
                t
            }
        }
    }

    /// Lowers `await e` (in a coroutine) to a suspend point: the future `e` is evaluated in the
    /// current block, which ends with a [`Terminator::Await`] parking the task; lowering continues in
    /// a fresh `resume` block where the settled result is bound to `dest`. Returns the `dest` read, so
    /// callers see `await e` as an ordinary value — hence awaits compose in any sub-expression.
    fn lower_await(&mut self, await_expr: &HExpr) -> Operand {
        let HExprKind::Await(inner) = &await_expr.kind else {
            unreachable!("lower_await on non-await expression");
        };
        let future = self.lower_operand(inner);
        let dest = self.b.new_temp(await_expr.ty);
        let resume = self.b.new_block();
        self.b.terminate(Terminator::Await {
            future,
            dest: Some(dest),
            resume,
        });
        self.b.switch_to(resume);
        Operand::Copy(Place::Local(dest))
    }

    fn lower_callee(&self, callee: &crate::hir::Callee) -> super::Callee {
        super::Callee {
            def: callee.def,
            args: callee.instance.clone(),
            ret: callee.ret,
        }
    }

    /// `a && b` / `a || b`: evaluate `b` only on the deciding branch, joining into one bool temp.
    fn lower_short_circuit(&mut self, e: &HExpr) -> Operand {
        let (op, lhs, rhs) = match &e.kind {
            HExprKind::Binary { op, lhs, rhs } => (*op, lhs, rhs),
            _ => unreachable!("lower_short_circuit on non-binary"),
        };
        let result = self.b.new_temp(e.ty);
        let l = self.lower_operand(lhs);

        let rhs_blk = self.b.new_block();
        let short_blk = self.b.new_block();
        let join = self.b.new_block();

        // `&&`: if lhs then evaluate rhs else result=false. `||`: if lhs then result=true else rhs.
        let (then_blk, else_blk) = if op == super::BinOp::And {
            (rhs_blk, short_blk)
        } else {
            (short_blk, rhs_blk)
        };
        self.b.terminate(Terminator::If {
            cond: l,
            then_blk,
            else_blk,
        });

        self.b.switch_to(short_blk);
        let short_val = op == super::BinOp::Or;
        self.b.assign(
            Place::Local(result),
            Rvalue::Use(Operand::Const(Const::Bool(short_val))),
        );
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(rhs_blk);
        let r = self.lower_operand(rhs);
        self.b.assign(Place::Local(result), Rvalue::Use(r));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    fn lower_ternary(&mut self, e: &HExpr) -> Operand {
        let (cond, then_e, else_e) = match &e.kind {
            HExprKind::Ternary {
                cond,
                then_expr,
                else_expr,
            } => (cond, then_expr, else_expr),
            _ => unreachable!(),
        };
        let result = self.b.new_temp(e.ty);
        let c = self.lower_operand(cond);
        let then_blk = self.b.new_block();
        let else_blk = self.b.new_block();
        let join = self.b.new_block();
        self.b.terminate(Terminator::If {
            cond: c,
            then_blk,
            else_blk,
        });

        self.b.switch_to(then_blk);
        let tv = self.lower_operand(then_e);
        self.b.assign(Place::Local(result), Rvalue::Use(tv));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(else_blk);
        let ev = self.lower_operand(else_e);
        self.b.assign(Place::Local(result), Rvalue::Use(ev));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    fn lower_coalesce(&mut self, e: &HExpr) -> Operand {
        // `lhs ?? rhs`: result = lhs unless lhs is null, then rhs.
        let (lhs, rhs) = match &e.kind {
            HExprKind::Coalesce { lhs, rhs } => (lhs, rhs),
            _ => unreachable!(),
        };
        let result = self.b.new_temp(e.ty);
        let l = self.operand_into_local(lhs);

        let is_null = self.b.new_temp(self.interner.bool());
        self.b.assign(
            Place::Local(is_null),
            Rvalue::Binary(
                super::BinOp::Eq,
                Operand::Copy(Place::Local(l)),
                Operand::Const(Const::Null),
            ),
        );
        let rhs_blk = self.b.new_block();
        let lhs_blk = self.b.new_block();
        let join = self.b.new_block();
        self.b.terminate(Terminator::If {
            cond: Operand::Copy(Place::Local(is_null)),
            then_blk: rhs_blk,
            else_blk: lhs_blk,
        });

        self.b.switch_to(lhs_blk);
        self.b.assign(
            Place::Local(result),
            Rvalue::Use(Operand::Copy(Place::Local(l))),
        );
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(rhs_blk);
        let rv = self.lower_operand(rhs);
        self.b.assign(Place::Local(result), Rvalue::Use(rv));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    fn lower_place(&mut self, place: &HPlace) -> Place {
        match place {
            HPlace::Local(l) => Place::Local(self.mir_local(*l)),
            HPlace::Global(g) => Place::Global(super::Global(g.0)),
            HPlace::Field { obj, field } => {
                let base = self.operand_into_local(obj);
                Place::Field {
                    base,
                    field: *field,
                }
            }
            HPlace::Index { array, index } => {
                let base = self.operand_into_local(array);
                let idx = self.lower_operand(index);
                Place::Index {
                    base,
                    index: Box::new(idx),
                }
            }
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
