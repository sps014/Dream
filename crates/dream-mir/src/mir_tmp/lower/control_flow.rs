//! `if`/`while`/`do-while`/`for`/`foreach` desugaring into block graphs, and `break`/`continue`
//! label resolution. Split out of `lower.rs`; these are methods on the parent's private `Lowerer`.

use super::*;

impl Lowerer<'_> {
    pub(super) fn lower_if(&mut self, cond: &HExpr, then_b: &[HStmt], else_b: &[HStmt]) {
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

    pub(super) fn lower_while(&mut self, cond: &HExpr, body: &[HStmt], label: Option<&str>) {
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
            lock_depth: self.locks.len(),
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
    pub(super) fn lower_do_while(&mut self, cond: &HExpr, body: &[HStmt], label: Option<&str>) {
        let body_blk = self.b.new_block();
        let cond_blk = self.b.new_block();
        let after_blk = self.b.new_block();
        self.b.terminate(Terminator::Goto(body_blk));

        self.loops.push(LoopCtx {
            break_blk: after_blk,
            continue_blk: cond_blk,
            label: label.map(str::to_string),
            lock_depth: self.locks.len(),
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

    pub(super) fn lower_for(
        &mut self,
        init: &[HStmt],
        cond: &HExpr,
        step: &[HStmt],
        body: &[HStmt],
        label: Option<&str>,
    ) {
        self.lower_block(init);
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
            lock_depth: self.locks.len(),
        });
        self.b.switch_to(body_blk);
        self.lower_block(body);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(step_blk));
        }
        self.loops.pop();

        self.b.switch_to(step_blk);
        self.lower_block(step);
        self.b.terminate(Terminator::Goto(cond_blk));

        self.b.switch_to(after_blk);
    }

    pub(super) fn lower_foreach(
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
                super::super::BinOp::Lt,
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
            lock_depth: self.locks.len(),
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
                super::super::BinOp::Add,
                Operand::Copy(Place::Local(idx)),
                Operand::Const(Const::Int(1)),
            ),
        );
        self.b.terminate(Terminator::Goto(cond_blk));

        self.b.switch_to(after_blk);
    }

    pub(super) fn lower_break(&mut self, label: Option<&str>) {
        if let Some((target, lock_depth)) = self.loop_target(label, true) {
            self.release_locks_above(lock_depth);
            self.b.terminate(Terminator::Goto(target));
        }
    }

    pub(super) fn lower_continue(&mut self, label: Option<&str>) {
        if let Some((target, lock_depth)) = self.loop_target(label, false) {
            self.release_locks_above(lock_depth);
            self.b.terminate(Terminator::Goto(target));
        }
    }

    fn loop_target(
        &self,
        label: Option<&str>,
        is_break: bool,
    ) -> Option<(super::super::BlockId, usize)> {
        let ctx = match label {
            Some(l) => self
                .loops
                .iter()
                .rev()
                .find(|c| c.label.as_deref() == Some(l)),
            None => self.loops.last(),
        }?;
        let blk = if is_break {
            ctx.break_blk
        } else {
            ctx.continue_blk
        };
        Some((blk, ctx.lock_depth))
    }

    /// `lock (target) { body }`: evaluates `target` exactly once into a fresh temp (so a
    /// `break`/`continue`/`return` out of `body` can release the same instance without
    /// re-evaluating `target`, which may have side effects), acquires its embedded lock word,
    /// lowers `body`, then releases on the normal fallthrough exit. `break`/`continue`/`return`
    /// release it (and any other lock entered inside `body`) on their own paths — see
    /// [`Self::release_locks_above`]/[`Self::release_all_locks`]. The temp's *declared* type (a
    /// `@shared class`) is what lets the backend find the target's lock-word offset
    /// (`obj_ptr + layout.size`, computed from the layout table at emission — see
    /// `Emitter::emit_stmt`'s `Statement::LockAcquire`/`LockRelease` arms) without needing the
    /// layout table here during HIR->MIR lowering.
    pub(super) fn lower_lock(&mut self, target: &HExpr, body: &[HStmt]) {
        let target_op = self.lower_operand(target);
        let ptr = self.b.new_temp(target.ty);
        self.b.assign(Place::Local(ptr), Rvalue::Use(target_op));
        self.b
            .push(Statement::LockAcquire(Operand::Copy(Place::Local(ptr))));
        self.locks.push(ptr);
        self.lower_block(body);
        self.locks.pop();
        if !self.b.is_terminated() {
            self.b
                .push(Statement::LockRelease(Operand::Copy(Place::Local(ptr))));
        }
    }

    /// Releases every lock in `self.locks` above `depth`, innermost (most recently acquired) first —
    /// used when a `break`/`continue` jumps out past one or more `lock` blocks nested inside the
    /// loop it targets, but leaves any lock already held before the loop was entered untouched.
    fn release_locks_above(&mut self, depth: usize) {
        for i in (depth..self.locks.len()).rev() {
            let addr = self.locks[i];
            self.b
                .push(Statement::LockRelease(Operand::Copy(Place::Local(addr))));
        }
    }

    /// Releases every currently-held lock, innermost first — used at `return`, which unwinds every
    /// `lock` scope on its way out of the function.
    pub(super) fn release_all_locks(&mut self) {
        self.release_locks_above(0);
    }
}
