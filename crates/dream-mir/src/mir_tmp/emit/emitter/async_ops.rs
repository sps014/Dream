//! Async-coroutine emission for the WAT backend: the poll-completion path, the state-machine
//! dispatch that drives an async body's suspend/resume, and the async CFG terminator / intrinsic
//! helpers. Split out of `emitter.rs`; these are methods on the parent's private `Emitter`.

use super::*;
use crate::mir::async_emit::AsyncSlots;

impl Emitter<'_> {
    /// Completes the current coroutine: releases the persistent user reference locals, then
    /// `$dream_complete($self, value)` and returns `0` (the poll result).
    fn emit_poll_complete(&mut self, value: Option<&Operand>) {
        if let Some(parent) = self.async_parent {
            // Only the persistent user locals (params + declared `let`s) are released; the trailing
            // synthetic temps are transient and have no guaranteed release helper for their types.
            for (i, decl) in parent
                .locals
                .iter()
                .enumerate()
                .take(self.async_user_locals)
            {
                if self.interner.is_reference(decl.ty) {
                    let call = release_call(self.interner, self.layouts, decl.ty);
                    self.line(&format!("     (local.get ${i})"));
                    self.line(&format!("     (call {call})"));
                }
            }
        }
        // Debug-info: the coroutine is finishing, so pop its shadow call-stack frame. This is the only
        // exit path (awaits return without popping), so the frame count stays balanced.
        self.emit_debug_exit();
        self.line("     (local.get $self)");
        match value {
            Some(v) => {
                self.emit_operand(v);
                // `Future.result` (`F_RESULT`) is a single `i32` slot: an `i32`-native value (int,
                // bool, char, or any reference — already a heap pointer) fits directly, but a wider
                // scalar (`long`/`float`/`double`) does not. Box it into a heap cell first so
                // `$dream_complete` (and every await-resume site below) only ever deals in `i32`.
                let wt = self.wasm_ty(self.operand_ty(v));
                match wt.as_str() {
                    "i64" => self.line("     (call $box_long)"),
                    "f32" => self.line("     (call $box_float)"),
                    "f64" => self.line("     (call $box_double)"),
                    _ => {}
                }
            }
            None => self.line("     (i32.const 0)"),
        }
        self.line("     (call $dream_complete)");
        self.line("     (i32.const 0)");
        self.line("     (return)");
    }

    /// Emits the coroutine poll function: a state-machine dispatch over the whole lowered async body.
    /// On entry the frame-resident locals are restored, then a `$__pc`/`br_table` loop (seeded from
    /// the saved `Future.state`) runs blocks; CFG edges re-dispatch, an [`Terminator::Await`] parks
    /// the task and returns (recording its `resume` block as the next state), and completions run
    /// `$dream_complete`. A block that is some await's `resume` target first binds the settled result.
    pub(super) fn emit_async_state_machine(&mut self, slots: &AsyncSlots, poll_sym: &str) {
        if self.debug {
            self.line(&format!(
                "(func ${} (@name \"{}__poll\") (param $self i32) (result i32)",
                poll_sym, self.func.name
            ));
        } else {
            self.line(&format!(
                "(func ${} (param $self i32) (result i32)",
                poll_sym
            ));
        }
        for (i, decl) in self.func.locals.iter().enumerate() {
            if let (true, Some(name)) = (self.debug, decl.name.as_ref()) {
                self.line(&format!(
                    " (local ${} (@name \"{}\") {})",
                    i,
                    name,
                    self.wasm_ty(decl.ty)
                ));
            } else {
                self.line(&format!(" (local ${} {})", i, self.wasm_ty(decl.ty)));
            }
        }
        // Scratch locals shared with the normal emitter (`$__obj`/`$__len`/`$__rel` back array &
        // reassignment scratch, `$__jsp` a saved `$__sp` across a dynamic `js` call, `$__src` the
        // source array/buffer pointer across a `T[]` `ToBytes`/`FromBytes` dynamic-length raw copy);
        // `$__pc` drives the block dispatch, `$__scratch` holds the awaited future at a suspend.
        self.line(" (local $__obj i32)");
        self.line(" (local $__scratch i32)");
        self.line(" (local $__len i32)");
        self.line(" (local $__rel i32)");
        self.line(" (local $__pc i32)");
        self.line(" (local $__jsp i32)");
        self.line(" (local $__src i32)");

        // Restore every frame-resident local; reference slots are zeroed after the move so ownership
        // lives in the WASM local (and is not double-freed from the frame) until the next suspend. A
        // value(`struct`) local's bytes live directly at its own fixed frame offset (see
        // `AsyncSlots::value_locals`), so its "value" is just that address, recomputed every poll —
        // never loaded/saved like a plain scalar (its inline storage persists in the frame on its own).
        for (idx, _, wt) in &slots.entries {
            let off = slots.offsets[idx];
            if slots.value_locals.contains_key(idx) {
                self.line(" local.get $self");
                self.line(&format!(" i32.const {}", off));
                self.line(" i32.add");
                self.line(&format!(" local.set ${}", idx));
                continue;
            }
            self.line(" local.get $self");
            self.line(&format!(" {} offset={}", slot_load(wt), off));
            self.line(&format!(" local.set ${}", idx));
            if slots.ref_locals.contains(idx) {
                self.line(" local.get $self");
                self.line(" i32.const 0");
                self.line(&format!(" i32.store offset={}", off));
            }
        }

        // Blocks that are an await's `resume` target, mapped to the local its result binds to (if any).
        let mut resume_binds: HashMap<u32, Option<crate::mir::Local>> = HashMap::new();
        for block in &self.func.blocks {
            if let Terminator::Await { dest, resume, .. } = &block.terminator {
                resume_binds.insert(resume.0, *dest);
            }
        }

        let n = self.func.blocks.len();
        self.line(" local.get $self");
        self.line(&format!(" i32.load offset={}", F_STATE));
        self.line(" local.set $__pc");
        // Debug-info: announce entry once, on the *initial* poll (state/pc still 0). Resume polls
        // (pc != 0, after an `await`) must not re-push the frame, and suspends must not pop it - the
        // frame is popped only on completion (see `emit_poll_complete`), keeping the shadow call
        // stack balanced across awaits.
        if let Some(dbg) = self.debug_fn {
            self.line(&format!(
                " (if (i32.eqz (local.get $__pc)) (then (call $__dbg_enter (i32.const {}))))",
                dbg.id
            ));
        }
        self.line(" (block $host_exit");
        self.line("  (loop $__loop");
        for i in (0..n).rev() {
            self.line(&format!("   (block $bb{}", i));
        }
        let labels: String = (0..n).map(|i| format!("$bb{} ", i)).collect();
        let default = format!("$bb{}", n.saturating_sub(1));
        self.line(&format!(
            "    (br_table {}{} (local.get $__pc))",
            labels, default
        ));
        for i in 0..n {
            self.line(&format!("   ) ;; bb{} body", i));
            if let Some(dest) = resume_binds.get(&(i as u32)) {
                // Resume point: bind the settled result (`awaiting.result`) before continuing. A
                // wide-scalar (`long`/`float`/`double`) result was boxed by the awaited coroutine's
                // `emit_poll_complete` (see above) since `F_RESULT` only ever holds an `i32`; unbox
                // it back to its native representation here and release the now-consumed box cell.
                let dest_wt = dest.map(|d| self.wasm_ty(self.func.local_ty(d)));
                self.line("     (local.get $self)");
                self.line(&format!("     (i32.load offset={})", F_AWAITING));
                self.line(&format!("     (i32.load offset={})", F_RESULT));
                match dest_wt.as_deref() {
                    Some("i64") | Some("f32") | Some("f64") => {
                        self.line("     (local.set $__obj)");
                        let unbox = match dest_wt.as_deref() {
                            Some("i64") => "$unbox_long",
                            Some("f32") => "$unbox_float",
                            _ => "$unbox_double",
                        };
                        self.line(&format!("     (call {unbox} (local.get $__obj))"));
                        if let Some(d) = dest {
                            self.line(&format!("     (local.set ${})", d.0));
                        }
                        self.line("     (local.get $__obj)");
                        self.line("     (call $release_generic)");
                    }
                    _ => match dest {
                        Some(d) => self.line(&format!("     (local.set ${})", d.0)),
                        None => self.line("     (drop)"),
                    },
                }
            }
            let block = self.func.block(crate::mir::BlockId(i as u32));
            for stmt in &block.stmts {
                self.emit_stmt(stmt);
            }
            self.emit_async_cfg_terminator(&block.terminator, slots);
        }
        self.line("  )"); // loop
        self.line(" )"); // $host_exit
                         // Every reachable path suspends (returns) or completes (returns); the tail is unreachable but
                         // keeps the `(result i32)` signature well-typed.
        self.line(" (unreachable)");
        self.line(")");
    }

    /// Terminator emission inside the coroutine poll dispatch: CFG edges re-dispatch through `$__pc`,
    /// an `Await` parks the task and returns, and completions/returns finish the task.
    fn emit_async_cfg_terminator(&mut self, t: &Terminator, slots: &AsyncSlots) {
        match t {
            Terminator::Goto(_) | Terminator::If { .. } | Terminator::Switch { .. } => {
                self.emit_terminator(t)
            }
            Terminator::Await { future, resume, .. } => {
                // Evaluate the awaited future, park the task on it, save live locals, and return so the
                // scheduler can drive it; the poll re-enters at `resume` when the future settles.
                self.emit_operand(future);
                self.line("     (local.set $__scratch)");
                self.line("     (local.get $self)");
                self.line("     (local.get $__scratch)");
                self.line(&format!("     (i32.store offset={})", F_AWAITING));
                self.line("     (local.get $self)");
                self.line(&format!("     (i32.const {})", resume.0));
                self.line(&format!("     (i32.store offset={})", F_STATE));
                for (idx, _, wt) in &slots.entries {
                    // A value(`struct`) local's bytes already live at their fixed frame offset (the
                    // local is just `self + offset`, recomputed on restore) — nothing to save.
                    if slots.value_locals.contains_key(idx) {
                        continue;
                    }
                    let off = slots.offsets[idx];
                    self.line("     (local.get $self)");
                    self.line(&format!("     (local.get ${})", idx));
                    self.line(&format!("     ({} offset={})", slot_store(wt), off));
                }
                self.line("     (local.get $self)");
                self.line("     (local.get $__scratch)");
                self.line("     (call $dream_await)");
                self.line("     (i32.const 0)");
                self.line("     (return)");
            }
            Terminator::AsyncComplete(v) => {
                let v = v.clone();
                self.emit_poll_complete(v.as_ref());
            }
            // A value `return x;` in an async body lowers to `AsyncComplete`; handle the plain form too.
            Terminator::Return(v) => {
                let v = v.clone();
                self.emit_poll_complete(v.as_ref());
            }
            // TCO never runs on async bodies, so a tail call cannot appear in a poll function.
            Terminator::TailCall { .. } => self.line("     (unreachable) ;; tail call in async fn"),
            Terminator::Unreachable => self.line("     (unreachable)"),
        }
    }

    /// Emits `sleep` / `Promise.all|any|race`, leaving a `Future` pointer on the stack.
    pub(super) fn emit_async_intrinsic(&mut self, kind: &str, args: &[Operand]) {
        use crate::intrinsics;
        match kind {
            intrinsics::SLEEP => {
                use crate::mir::async_emit::{F_SLOTS, HOST_POLL_INDEX, KIND_HOST};
                self.emit_operand(&args[0]);
                self.line("     (local.set $__scratch)");
                self.line(&format!("     (i32.const {F_SLOTS}) ;; F_SLOTS"));
                self.line(&format!("     (i32.const {HOST_POLL_INDEX})"));
                self.line(&format!("     (i32.const {KIND_HOST}) ;; KIND_HOST"));
                self.line("     (call $dream_new_future)");
                self.line("     (local.tee $__obj)");
                self.line("     (local.get $__scratch)");
                self.line("     (call $dream_set_timer)");
                self.line("     (local.get $__obj)");
            }
            intrinsics::PROMISE_ALL => {
                self.emit_operand(&args[0]);
                self.line("     (call $dream_all)");
            }
            intrinsics::PROMISE_ANY | intrinsics::PROMISE_RACE => {
                self.emit_operand(&args[0]);
                self.line("     (call $dream_any)");
            }
            _ => {}
        }
    }
}
