//! `Terminator` emission (branches, returns, tail calls) for the WAT backend, plus the dynamic `js`
//! call marshaling helper. Split out of `emitter.rs`; these are methods on the parent's private
//! `Emitter`.

use super::*;

impl Emitter<'_> {
    pub(super) fn emit_terminator(&mut self, t: &Terminator) {
        match t {
            Terminator::Goto(b) => self.goto(*b),
            Terminator::If {
                cond,
                then_blk,
                else_blk,
            } => {
                self.emit_operand(cond);
                self.line("     (if (then");
                self.goto(*then_blk);
                self.line("     ) (else");
                self.goto(*else_blk);
                self.line("     ))");
            }
            Terminator::Switch {
                value,
                targets,
                default,
            } => {
                // Lower to a chain of compares; a real br_table needs contiguous keys.
                for (k, b) in targets {
                    self.emit_operand(value);
                    self.line(&format!("     (i32.const {})", k));
                    self.line("     (i32.eq)");
                    self.line("     (if (then");
                    self.goto(*b);
                    self.line("     ))");
                }
                self.goto(*default);
            }
            Terminator::Return(Some(o)) => {
                self.emit_debug_exit();
                if self.returns_value_struct() {
                    // sret ABI: copy the result into the caller-provided `$__sret` slot (retaining
                    // its reference fields) before the frame teardown drops the source local.
                    let o = o.clone();
                    let ty = self.func.ret;
                    self.emit_value_copy(
                        |s| s.line("     (local.get $__sret)"),
                        |s| s.emit_operand_addr(&o),
                        ty,
                    );
                    self.emit_frame_teardown();
                    self.line("     (return)");
                } else {
                    self.emit_operand(o);
                    self.emit_frame_teardown();
                    self.line("     (return)");
                }
            }
            Terminator::Return(None) => {
                self.emit_debug_exit();
                self.emit_frame_teardown();
                self.line("     (return)");
            }
            Terminator::TailCall { callee, args } => {
                self.emit_debug_exit();
                let sym = self.callee_symbol(callee);
                if let Some(kind) = async_intrinsic_kind(&sym) {
                    // Async intrinsics have a bespoke calling convention and can't be tail-called;
                    // fall back to `f(args); return`. (The `tco` pass avoids this, so it is only a
                    // safety net.)
                    self.emit_async_intrinsic(kind, args);
                    if !self.wasm_returns_value() {
                        self.line("     (drop)");
                    }
                    self.emit_frame_teardown();
                    self.line("     (return)");
                } else {
                    // Arguments are all scalar (the pass guarantees no value-struct/sret ABI), so
                    // the frame teardown below never touches them: it only drops this frame's inline
                    // value-struct slots and restores `$__sp`, leaving the pushed args intact for
                    // `return_call` to consume.
                    self.emit_call_args(callee, args);
                    self.emit_frame_teardown();
                    self.line(&format!("     (return_call ${})", sym));
                }
            }
            Terminator::Unreachable => self.line("     (unreachable)"),
            Terminator::AsyncComplete(_) => self.line("     (unreachable) ;; async in sync fn"),
            Terminator::Await { .. } => self.line("     (unreachable) ;; await in sync fn"),
        }
    }

    /// A CFG edge: set the dispatch PC to the target and loop back to re-dispatch.
    fn goto(&mut self, target: crate::mir::BlockId) {
        self.line(&format!("     (i32.const {})", target.0));
        self.line("     (local.set $__pc)");
        self.line("     (br $__loop)");
    }

    /// Emits a dynamic `js` call marshaling its arguments through the shadow stack in one host
    /// crossing (no per-argument boxing, no heap array): save `$__sp`, carve `argc * 16` bytes,
    /// write one 16-byte tagged slot per argument (`[tag][aux][payload]`), call the bridge with
    /// `(target, [namePtr,] argsPtr, argc)`, and restore `$__sp` (the `i32` result handle stays on
    /// the WASM stack). The buffer lives below the value-struct frame and is released immediately, so
    /// it is allocation-free and re-entrant (a nested `js` call saves/restores its own `$__sp`).
    pub(super) fn emit_js_call(
        &mut self,
        callee: &crate::mir::Callee,
        target: &Operand,
        method: Option<&Operand>,
        args: &[(Operand, TypeId)],
    ) {
        use crate::mir::js_abi;
        let argc = args.len() as u32;
        // Save `$__sp` and carve the slot buffer (skipped for a zero-argument call).
        self.line("     (global.get $__sp) (local.set $__jsp)");
        if argc > 0 {
            self.line(&format!(
                "     (global.get $__sp) (i32.const {}) (i32.sub) (global.set $__sp)",
                argc * js_abi::SLOT_SIZE
            ));
        }
        for (i, (op, ty)) in args.iter().enumerate() {
            let base = (i as u32) * js_abi::SLOT_SIZE;
            let (tag, aux, store) = js_abi::slot_desc(self.interner, *ty);
            self.emit_slot_word(base, &format!("(i32.const {})", tag));
            self.emit_slot_word(
                base + js_abi::SLOT_AUX_OFFSET,
                &format!("(i32.const {})", aux),
            );
            // Payload: the argument value, stored at its natural width.
            self.line(&format!(
                "     (global.get $__sp) (i32.const {}) (i32.add)",
                base + js_abi::SLOT_PAYLOAD_OFFSET
            ));
            self.emit_operand(op);
            self.line(&format!("     ({})", store));
        }
        // Bridge args: target, [namePtr,] argsPtr (= current $__sp), argc.
        self.emit_operand(target);
        if let Some(name) = method {
            self.emit_operand(name);
        }
        self.line("     (global.get $__sp)");
        self.line(&format!("     (i32.const {})", argc));
        let sym = self.callee_symbol(callee);
        self.line(&format!("     (call ${sym})"));
        // Release the buffer; the call's `i32` result remains beneath on the WASM stack.
        self.line("     (local.get $__jsp) (global.set $__sp)");
    }

    /// Stores an `i32` `value` (a WAT snippet leaving one `i32` on the stack) into the argument-slot
    /// buffer at byte offset `off` from `$__sp` — used by [`emit_js_call`](Self::emit_js_call) for a
    /// slot's `tag`/`aux` header words.
    fn emit_slot_word(&mut self, off: u32, value: &str) {
        self.line(&format!(
            "     (global.get $__sp) (i32.const {}) (i32.add)",
            off
        ));
        self.line(&format!("     {} (i32.store)", value));
    }
}
