//! Call-argument and call-shape emission: direct-call argument widening, dynamic interface dispatch
//! (value and sret ABIs), and indirect sret dispatch. Methods on the parent module's private
//! `Emitter`; the ones reached from sibling emitter modules (statements/terminator/value_struct) are
//! widened to the emitter-module scope.

use super::*;

impl Emitter<'_> {
    /// Emits a call's arguments, applying implicit numeric widening to each so a narrower argument
    /// (e.g. an `int`/`float` passed to a `double` parameter) matches the callee's WASM signature.
    /// Falls back to a plain push when the callee's parameter types are unknown (imports/intrinsics).
    pub(in crate::mir::emit::emitter) fn emit_call_args(
        &mut self,
        callee: &crate::mir::Callee,
        args: &[Operand],
    ) {
        let params = self.sigs.get(&(callee.def, callee.args.clone())).cloned();
        for (i, a) in args.iter().enumerate() {
            self.emit_operand(a);
            if let Some(pty) = params.as_ref().and_then(|p| p.get(i)) {
                self.emit_numeric_conv(self.operand_ty(a), *pty);
            }
        }
    }

    /// Emits a dynamic interface method call. The receiver is pushed as argument 0, then the real
    /// arguments (widened to the interface method's declared parameter types), then control transfers
    /// to the per-`(interface, method)` dispatch trampoline which looks the concrete implementation up
    /// in the tag-indexed itable and forwards through `$__ft`. The trampoline leaves the result (if
    /// any) on the stack.
    pub(in crate::mir::emit::emitter) fn emit_interface_call(
        &mut self,
        receiver: &Operand,
        iface_id: usize,
        method_slot: usize,
        sig: TypeId,
        args: &[Operand],
    ) {
        self.emit_interface_receiver_args(receiver, sig, args);
        self.line(&format!(
            "     (call ${})",
            iface_dispatch_symbol(iface_id, method_slot)
        ));
    }

    /// Emits a dynamic interface call to a *value*(`struct`/union)-returning method using the sret
    /// ABI: the destination address (produced by `dst`) is pushed as the hidden leading argument,
    /// then the receiver and real arguments, then control transfers to the dispatch trampoline
    /// (which forwards the sret pointer through to the concrete implementation). Mirrors
    /// [`emit_value_sret_call`](Self::emit_value_sret_call) for direct calls.
    pub(in crate::mir::emit::emitter) fn emit_interface_sret_call(
        &mut self,
        dst: impl Fn(&mut Self),
        receiver: &Operand,
        iface_id: usize,
        method_slot: usize,
        sig: TypeId,
        args: &[Operand],
    ) {
        dst(self);
        self.emit_interface_receiver_args(receiver, sig, args);
        self.line(&format!(
            "     (call ${})",
            iface_dispatch_symbol(iface_id, method_slot)
        ));
    }

    /// Pushes an interface call's receiver (argument 0) followed by the real arguments, each widened
    /// to the interface method's declared parameter type. Shared by the value and sret interface-call
    /// paths, which differ only in whether an sret destination precedes the receiver.
    fn emit_interface_receiver_args(&mut self, receiver: &Operand, sig: TypeId, args: &[Operand]) {
        let param_tys: Vec<TypeId> = match self.interner.kind(sig) {
            TyKind::Func(params, _) => params.clone(),
            _ => Vec::new(),
        };
        self.emit_operand(receiver);
        for (i, a) in args.iter().enumerate() {
            self.emit_operand(a);
            // param_tys[0] is the receiver (`this`); real args start at index 1.
            if let Some(pty) = param_tys.get(i + 1) {
                self.emit_numeric_conv(self.operand_ty(a), *pty);
            }
        }
    }

    /// Emits an indirect (funcref) call to a value-struct-returning target using the sret ABI: the
    /// destination address (`dst`) is passed as the hidden leading argument, then the real
    /// arguments, then the table index dispatched through `$__ft` with the target's sret signature.
    /// Mirrors [`emit_value_sret_call`](Self::emit_value_sret_call) for first-class function values
    /// (e.g. a worker body funcref of type `fun(TIn): TOut` where `TOut` is a struct).
    pub(in crate::mir::emit::emitter) fn emit_indirect_sret_call(
        &mut self,
        dst: impl Fn(&mut Self),
        target: &Operand,
        args: &[Operand],
    ) {
        dst(self);
        for a in args {
            self.emit_operand(a);
        }
        self.emit_operand(target);
        let sig = func_sig(self.interner, self.operand_ty(target))
            .map(|(name, _, _)| name)
            .unwrap_or_else(|| "$sig___v".to_string());
        self.line(&format!("     (call_indirect $__ft (type {}))", sig));
    }
}
