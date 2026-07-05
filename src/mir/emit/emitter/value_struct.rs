//! The value(`struct`)/value-union inline (non-heap) representation for the WAT backend: the
//! per-function shadow-frame prologue/teardown, in-place construction (`New`/`UnionNew`), byte-wise
//! copy/drop, and sret-call helpers. Split out of `emitter.rs`; these are methods on the parent's
//! private `Emitter`.

use super::*;

impl Emitter<'_> {
    /// Reserves this function's shadow-stack frame (for inline value(`struct`) locals): save `$__sp`,
    /// carve the frame by growing the stack downward, zero it (so drop-glue on a not-yet-assigned slot
    /// sees null reference fields), and point each owning value local at its slot.
    pub(super) fn emit_value_frame_prologue(&mut self) {
        if self.frame.size == 0 {
            return;
        }
        let size = self.frame.size;
        self.line("     (global.get $__sp) (local.set $__saved_sp)");
        self.line(&format!(
            "     (global.get $__sp) (i32.const {}) (i32.sub) (global.set $__sp)",
            size
        ));
        // Zero the whole frame: memory.fill(dest = $__sp, value = 0, len = size).
        self.line(&format!(
            "     (global.get $__sp) (i32.const 0) (i32.const {}) (memory.fill)",
            size
        ));
        for (local, offset) in self.frame.owning_slots() {
            let ty = self.func.local_ty(local);
            let l0 = local.0;
            let slot_addr = |s: &mut Self| {
                s.line("     (global.get $__sp)");
                if offset > 0 {
                    s.line(&format!("     (i32.const {}) (i32.add)", offset));
                }
            };
            if self.frame.kind(local) == Some(ValueLocalKind::Param) {
                // A value param arrives as a pointer to the caller's value: copy those bytes into the
                // callee's private slot (retaining reference fields), then rebind the param to the slot
                // so the caller's value is never mutated (copy semantics).
                self.emit_value_copy(
                    slot_addr,
                    |s| s.line(&format!("     (local.get ${})", l0)),
                    ty,
                );
            }
            slot_addr(self);
            self.line(&format!("     (local.set ${})", l0));
        }
    }

    /// True when this function returns a value struct by the sret ABI (a hidden `$__sret` pointer)
    /// rather than as a WASM result.
    pub(super) fn returns_value_struct(&self) -> bool {
        self.interner.is_value_type(self.func.ret)
    }

    /// True when this function returns an ordinary WASM value (non-void, non-value-struct).
    pub(super) fn wasm_returns_value(&self) -> bool {
        !matches!(self.interner.kind(self.func.ret), TyKind::Void) && !self.returns_value_struct()
    }

    /// The inline byte size of value struct `ty`.
    pub(super) fn value_size(&self, ty: TypeId) -> u32 {
        scalar_size(self.interner, ty).0
    }

    /// True when value struct `ty` needs retain/drop glue (embeds references or declares `del`).
    pub(super) fn value_has_glue(&self, ty: TypeId) -> bool {
        self.value_glue.contains(&self.interner.strip_nullable(ty))
    }

    /// The layout name of value type `ty` (a value struct or value union), if laid out. Used to name
    /// its retain/drop glue.
    pub(super) fn value_name(&self, ty: TypeId) -> Option<String> {
        let stripped = self.interner.strip_nullable(ty);
        if let Some(l) = self.layouts.get(stripped) {
            return Some(l.name.clone());
        }
        self.layouts.union(stripped).map(|u| u.name.clone())
    }

    /// Pushes the address of value place `p` (a value struct is addressed, never loaded).
    fn emit_place_addr(&mut self, p: &Place) {
        match p {
            Place::Local(l) => self.line(&format!("     (local.get ${})", l.0)),
            Place::Field { base, field } => {
                if let Some((off, _)) = self.field_layout(*base, *field) {
                    self.field_addr(*base, off);
                }
            }
            Place::Index { base, index } => {
                if let Some(ety) = self.array_elem_ty(*base) {
                    self.elem_addr(*base, ety, index);
                }
            }
            Place::Global(_) => self.line("     (i32.const 0) ;; value-struct global unsupported"),
        }
    }

    /// Pushes the address of a value-struct operand.
    pub(super) fn emit_operand_addr(&mut self, o: &Operand) {
        match o {
            Operand::Copy(p) => self.emit_place_addr(p),
            Operand::Const(_) => self.line("     (i32.const 0)"),
        }
    }

    /// Byte-wise copies value struct `ty` from the `src` address to the `dst` address, then retains
    /// the destination's (now duplicated) reference fields so the copy owns its own references.
    pub(super) fn emit_value_copy(
        &mut self,
        dst: impl Fn(&mut Self),
        src: impl Fn(&mut Self),
        ty: TypeId,
    ) {
        let size = self.value_size(ty);
        dst(self);
        src(self);
        self.line(&format!("     (i32.const {})", size));
        self.line("     (memory.copy)");
        if self.value_has_glue(ty) {
            if let Some(name) = self.value_name(ty) {
                dst(self);
                self.line(&format!("     (call {})", vs_retain_sym(&name)));
            }
        }
    }

    /// Drops the value struct `ty` at the `at` address (runs `del`, releases reference fields), if it
    /// needs glue.
    fn emit_value_drop(&mut self, at: impl Fn(&mut Self), ty: TypeId) {
        if self.value_has_glue(ty) {
            if let Some(name) = self.value_name(ty) {
                at(self);
                self.line(&format!("     (call {})", vs_drop_sym(&name)));
            }
        }
    }

    /// Constructs a value struct in place at the `dst` address: zero its bytes, then (if it has a
    /// user constructor) call `ctor(this = dst, args...)`.
    fn construct_value_new(
        &mut self,
        dst: impl Fn(&mut Self),
        ctor: Option<DefId>,
        args: &[Operand],
        ty: TypeId,
    ) {
        let size = self.value_size(ty);
        dst(self);
        self.line("     (i32.const 0)");
        self.line(&format!("     (i32.const {})", size));
        self.line("     (memory.fill)");
        if let Some(ctor) = ctor {
            dst(self);
            for arg in args {
                self.emit_operand(arg);
            }
            let sym = self.callee_symbol(&crate::mir::Callee {
                def: ctor,
                args: vec![],
                ret: self.interner.void(),
            });
            self.line(&format!("     (call ${})", sym));
        }
    }

    /// Emits a direct call to a value-struct-returning function using the sret ABI: the destination
    /// address (produced by `dst`) is passed as the hidden leading argument, then the real arguments.
    fn emit_value_sret_call(
        &mut self,
        dst: impl Fn(&mut Self),
        callee: &crate::mir::Callee,
        args: &[Operand],
    ) {
        dst(self);
        self.emit_call_args(callee, args);
        self.line(&format!("     (call ${})", self.callee_symbol(callee)));
    }

    /// Constructs a value union in place at the `dst` address: zero the block, write the variant
    /// discriminant at offset 0, then store each payload argument at its variant field offset (a
    /// value payload is copied inline; a reference payload is stored and retained).
    fn construct_value_union(
        &mut self,
        dst: impl Fn(&mut Self),
        ty: TypeId,
        variant: usize,
        args: &[Operand],
    ) {
        let size = self.value_size(ty);
        dst(self);
        self.line("     (i32.const 0)");
        self.line(&format!("     (i32.const {})", size));
        self.line("     (memory.fill)");
        dst(self);
        self.line(&format!("     (i32.const {}) ;; discriminant", variant));
        self.line("     (i32.store)");
        let fields: Vec<(u32, TypeId)> = self
            .layouts
            .union(ty)
            .and_then(|u| {
                u.variants
                    .iter()
                    .find(|v| v.discriminant as usize == variant)
                    .map(|v| v.fields.iter().map(|f| (f.offset, f.ty)).collect())
            })
            .unwrap_or_default();
        for (i, arg) in args.iter().enumerate() {
            let Some(&(off, fty)) = fields.get(i) else {
                continue;
            };
            let field_addr = |s: &mut Self| {
                dst(s);
                if off > 0 {
                    s.line(&format!("     (i32.const {}) (i32.add)", off));
                }
            };
            if self.interner.is_value_type(fty) {
                let arg = arg.clone();
                self.emit_value_copy(field_addr, |s| s.emit_operand_addr(&arg), fty);
            } else {
                field_addr(self);
                self.emit_operand(arg);
                self.line(&format!("     ({})", self.store_instr(fty)));
                self.retain_container_value(fty, arg);
            }
        }
    }

    /// Stores a value struct or value union produced by `rvalue` into the destination at the `dst`
    /// address (a local slot, a container field/element, or a union payload): the old contents are
    /// dropped, then the new value is constructed / sret-called / copied in place.
    pub(super) fn emit_value_store(
        &mut self,
        dst: impl Fn(&mut Self),
        ty: TypeId,
        rvalue: &Rvalue,
    ) {
        self.emit_value_drop(&dst, ty);
        match rvalue {
            Rvalue::New {
                ctor,
                args,
                ty: nty,
                ..
            } => self.construct_value_new(&dst, *ctor, args, *nty),
            Rvalue::UnionNew {
                ty: uty,
                variant,
                args,
                ..
            } => self.construct_value_union(&dst, *uty, *variant, args),
            Rvalue::Call { callee, args } => self.emit_value_sret_call(&dst, callee, args),
            Rvalue::IndirectCall { target, args } => {
                self.emit_indirect_sret_call(&dst, target, args)
            }
            Rvalue::InterfaceCall {
                receiver,
                iface_id,
                method_slot,
                sig,
                args,
                ..
            } => self.emit_interface_sret_call(&dst, receiver, *iface_id, *method_slot, *sig, args),
            Rvalue::Use(Operand::Copy(src)) => {
                let src = src.clone();
                self.emit_value_copy(&dst, |s| s.emit_place_addr(&src), ty);
            }
            other => {
                // Any other value-struct-producing rvalue (e.g. a `UnionField` payload extraction)
                // yields the *address* of an existing value; copy those bytes into the destination.
                let other = other.clone();
                self.emit_value_copy(&dst, |s| s.emit_rvalue(&other), ty);
            }
        }
    }

    /// Emits the scope-exit teardown of a function's shadow frame: drop each owning value local, then
    /// restore `$__sp`. A no-op for functions with no value frame.
    pub(super) fn emit_frame_teardown(&mut self) {
        for (local, _) in self.frame.owning_slots() {
            let ty = self.func.local_ty(local);
            let l0 = local.0;
            self.emit_value_drop(|s| s.line(&format!("     (local.get ${})", l0)), ty);
        }
        if self.frame.size > 0 {
            self.line("     (local.get $__saved_sp) (global.set $__sp)");
        }
    }
}
