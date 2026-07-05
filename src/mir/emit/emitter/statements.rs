//! `Statement` emission (assignment, print, retain/release, calls) for the WAT backend, plus the
//! place-store helpers used by assignment and by object-construction stores in `rvalue.rs`: retain
//! on store (`retain_container_value`), deferred release of an overwritten reference
//! (`stash_old_ref`/`release_stash`), and debug-info local spilling. Split out of `emitter.rs`;
//! these are methods on the parent's private `Emitter`.

use super::*;

impl Emitter<'_> {
    pub(super) fn emit_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(place, rvalue) => self.emit_assign(place, rvalue),
            Statement::Retain(o) => {
                self.emit_operand(o);
                self.line("     (call $retain)");
            }
            Statement::Release(o) => {
                // Deep release by the operand's declared type: structs/unions/reference arrays run
                // their generated `$release_<...>` (freeing fields + `del()`); other references fall
                // back to the generic/tag-dispatched runtime.
                let ty = self.operand_ty(o);
                let call = if self.interner.is_reference(ty) {
                    release_call(self.interner, self.layouts, ty)
                } else {
                    "$release_generic".to_string()
                };
                self.emit_operand(o);
                self.line(&format!("     (call {})", call));
            }
            Statement::Call { callee, args } => {
                self.emit_call_args(callee, args);
                self.line(&format!("     (call ${})", self.callee_symbol(callee)));
                if !matches!(self.interner.kind(callee.ret), TyKind::Void) {
                    self.line("     (drop)");
                }
            }
            Statement::InterfaceCall {
                receiver,
                iface_id,
                method_slot,
                sig,
                args,
            } => {
                self.emit_interface_call(receiver, *iface_id, *method_slot, *sig, args);
                let ret = match self.interner.kind(*sig) {
                    TyKind::Func(_, r) => Some(*r),
                    _ => None,
                };
                let drops = ret
                    .map(|r| !matches!(self.interner.kind(r), TyKind::Void))
                    .unwrap_or(false);
                if drops {
                    self.line("     (drop)");
                }
            }
            Statement::Print { arg, ty, newline } => {
                // Push the value, then print it. `int`/`char`/`string` go straight to a host import;
                // every other scalar is first rendered with its in-wasm `*_to_string` and printed as a
                // string. `println` appends a trailing newline (`\n` = 10) via `$print_char`.
                self.emit_operand(arg);
                match self.interner.kind(self.interner.strip_nullable(*ty)) {
                    TyKind::Prim(PrimTy::Int) => self.line("     (call $print_int)"),
                    TyKind::Prim(PrimTy::Char) => self.line("     (call $print_char)"),
                    TyKind::Prim(PrimTy::String) => self.line("     (call $print_string)"),
                    TyKind::Prim(prim) => {
                        let to_string = match prim {
                            PrimTy::Bool => "$bool_to_string",
                            PrimTy::Float => "$float_to_string",
                            PrimTy::Double => "$double_to_string",
                            PrimTy::Long => "$long_to_string",
                            PrimTy::UInt => "$uint_to_string",
                            PrimTy::ULong => "$ulong_to_string",
                            PrimTy::Byte => "$byte_to_string",
                            // Int/Char/String handled above; any other primitive prints via $print_int.
                            _ => "",
                        };
                        if to_string.is_empty() {
                            self.line("     (call $print_int)");
                        } else {
                            self.line(&format!("     (call {})", to_string));
                            self.line("     (call $print_string)");
                        }
                    }
                    // Enums are `i32` values at runtime; print their numeric value.
                    TyKind::Enum(_) => self.line("     (call $print_int)"),
                    // Arrays aren't self-describing at runtime (the header only says `TAG_ARRAY`), so
                    // the element-typed `to_string` is chosen statically here, then printed.
                    TyKind::Array(elem) => {
                        self.line(&format!("     (call {})", array_to_string_sym(*elem)));
                        self.line("     (call $print_string)");
                    }
                    // A value struct/union has no heap tag header, so it is rendered by its concrete
                    // `$<Type>_to_string` (chosen statically from the operand's type) and printed.
                    _ if self.interner.is_value_type(*ty) => {
                        if let Some(name) = self.value_name(*ty) {
                            self.line(&format!("     (call ${}_to_string)", name));
                            self.line("     (call $print_string)");
                        } else {
                            self.line("     (call $print_object)");
                        }
                    }
                    // Reference structs, unions, and `object` render through the tag-dispatching
                    // `$print_object` (which routes to each type's `to_string`).
                    _ => self.line("     (call $print_object)"),
                }
                if *newline {
                    self.line("     (i32.const 10)");
                    self.line("     (call $print_char)");
                }
            }
            Statement::Nop => {}
            Statement::DebugLine(line) => self.emit_debug_line(*line),
        }
    }

    /// Emits the debug-info instrumentation for a source-line boundary: spill every named local into
    /// the exported `$__dbg_v{k}` global pool (so the host debugger can read live values), then call
    /// the `dream_debug.line` host hook with `(file_id, line)`. A no-op unless debug-info is on for
    /// this function.
    fn emit_debug_line(&mut self, line: u32) {
        let Some(dbg) = self.debug_fn else {
            return;
        };
        let file_id = dbg.file;
        // Snapshot the spill descriptors so we can borrow `self` mutably while emitting.
        let vars: Vec<(u32, u32, crate::mir::emit::debug_map::SpillKind)> = dbg
            .vars
            .iter()
            .map(|v| (v.local, v.global, v.spill))
            .collect();
        for (local, global, kind) in vars {
            self.emit_var_spill(local, global, kind);
        }
        self.line(&format!(
            "     (call $__dbg_line (i32.const {}) (i32.const {}))",
            file_id, line
        ));
    }

    /// Spills one named local into its `i64` pool global, widening/reinterpreting to preserve the
    /// exact bits so the host can decode the value back using the variable's declared kind.
    fn emit_var_spill(
        &mut self,
        local: u32,
        global: u32,
        kind: crate::mir::emit::debug_map::SpillKind,
    ) {
        use crate::mir::emit::debug_map::SpillKind as K;
        let value = match kind {
            K::I64 => format!("(local.get ${})", local),
            K::F64 => format!("(i64.reinterpret_f64 (local.get ${}))", local),
            K::F32 => format!(
                "(i64.extend_i32_u (i32.reinterpret_f32 (local.get ${})))",
                local
            ),
            // i32 locals (ints, bools, chars, enums, string/aggregate/reference pointers): keep the
            // exact 32 bits via an unsigned extend.
            K::I32 => format!("(i64.extend_i32_u (local.get ${}))", local),
        };
        self.line(&format!("     (global.set $__dbg_v{} {})", global, value));
    }

    fn emit_assign(&mut self, place: &Place, rvalue: &Rvalue) {
        match place {
            Place::Local(l) => {
                let ty = self.func.local_ty(*l);
                if self.interner.is_value_type(ty) {
                    let l0 = l.0;
                    match self.frame.kind(*l) {
                        Some(ValueLocalKind::Owning) => {
                            self.emit_value_store(
                                |s| s.line(&format!("     (local.get ${})", l0)),
                                ty,
                                rvalue,
                            );
                        }
                        // A borrow/param value local just holds an address: rebind it to the source
                        // value's address (no copy, no drop).
                        _ => {
                            match rvalue {
                                Rvalue::Use(o) => self.emit_operand_addr(o),
                                _ => self.emit_rvalue(rvalue),
                            }
                            self.line(&format!("     (local.set ${})", l0));
                        }
                    }
                    return;
                }
                self.emit_rvalue(rvalue);
                self.line(&format!("     (local.set ${})", l.0));
            }
            Place::Global(g) => {
                self.emit_rvalue(rvalue);
                self.line(&format!("     (global.set $g{})", g.0));
            }
            Place::Field { base, field } => {
                if let Some((off, fty)) = self.field_layout(*base, *field) {
                    let (b, off, fty) = (*base, off, fty);
                    if self.interner.is_value_type(fty) {
                        self.emit_value_store(move |s| s.field_addr(b, off), fty, rvalue);
                        return;
                    }
                    let stash = self.stash_old_ref(fty, |s| s.field_addr(b, off));
                    self.field_addr(*base, off);
                    self.emit_rvalue(rvalue);
                    self.line(&format!("     ({})", self.store_instr(fty)));
                    self.retain_stored_rvalue(fty, rvalue);
                    self.release_stash(fty, stash);
                } else {
                    crate::internal_error!(
                        "missing field layout for store (base {:?}, field {})",
                        base,
                        field
                    );
                }
            }
            Place::Index { base, index } => {
                if let Some(ety) = self.array_elem_ty(*base) {
                    let (b, idx) = (*base, index.clone());
                    if self.interner.is_value_type(ety) {
                        let idx2 = idx.clone();
                        self.emit_value_store(move |s| s.elem_addr(b, ety, &idx2), ety, rvalue);
                        return;
                    }
                    let stash = self.stash_old_ref(ety, |s| s.elem_addr(b, ety, &idx));
                    self.elem_addr(*base, ety, index);
                    self.emit_rvalue(rvalue);
                    self.line(&format!("     ({})", self.store_instr(ety)));
                    self.retain_stored_rvalue(ety, rvalue);
                    self.release_stash(ety, stash);
                } else {
                    crate::internal_error!(
                        "missing array element type for store (base {:?})",
                        base
                    );
                }
            }
        }
    }

    /// Stores `value` into the object under construction (`$__obj + offset`) with the field/element
    /// width. Used by `New`/`ArrayLit` initialization. A *borrowed* reference (a copy of an existing
    /// place) is retained, since the container becomes a new owner; an owned producer is not
    /// materialized here (lowering routes those through a temporary that is itself released at scope
    /// exit), so retaining a copied operand is the sound, uniform rule.
    pub(super) fn store_at_obj(&mut self, offset: u32, value_ty: TypeId, value: &Operand) {
        // A value struct stored into a freshly-allocated container is copied inline (byte-wise + a
        // retain of its reference fields); the block was just zeroed, so there is no old value to
        // drop.
        if self.interner.is_value_type(value_ty) {
            let value = value.clone();
            self.emit_value_copy(
                |s| {
                    s.line("     (local.get $__obj)");
                    if offset > 0 {
                        s.line(&format!("     (i32.const {}) (i32.add)", offset));
                    }
                },
                |s| s.emit_operand_addr(&value),
                value_ty,
            );
            return;
        }
        self.line("     (local.get $__obj)");
        if offset > 0 {
            self.line(&format!("     (i32.const {})", offset));
            self.line("     (i32.add)");
        }
        self.emit_operand(value);
        self.line(&format!("     ({})", self.store_instr(value_ty)));
        self.retain_container_value(value_ty, value);
    }

    /// Emits a `$retain` of a reference value being stored into a container (struct field, array
    /// element, or union payload), so the container owns its own reference count. A no-op for
    /// non-reference values and for non-place operands (constants/null; `$retain` also null-guards).
    pub(super) fn retain_container_value(&mut self, value_ty: TypeId, value: &Operand) {
        let borrowed = matches!(value, Operand::Copy(_) | Operand::Const(Const::Str(_)));
        if self.interner.is_reference(value_ty) && borrowed {
            self.emit_operand(value);
            self.line("     (call $retain)");
        }
    }

    /// Before a reference field/element is overwritten, load and stash its previous occupant into the
    /// `$__rel` scratch so it can be released *after* the new value is stored (a deferred release keeps
    /// self-referential reassignments like `n.next = f(n.next)` sound). `emit_addr` pushes the slot's
    /// address. Returns `true` when a value was stashed (the slot is a reference). A no-op for
    /// non-reference slots, and releasing a null previous value (fresh field) is a runtime no-op.
    fn stash_old_ref(&mut self, ty: TypeId, emit_addr: impl Fn(&mut Self)) -> bool {
        if !self.interner.is_reference(ty) {
            return false;
        }
        emit_addr(self);
        self.line("     (i32.load)");
        self.line("     (local.set $__rel)");
        true
    }

    /// Releases the value stashed by [`Self::stash_old_ref`] (the overwritten field/element's previous
    /// occupant), if any.
    fn release_stash(&mut self, ty: TypeId, stashed: bool) {
        if !stashed {
            return;
        }
        let call = release_call(self.interner, self.layouts, ty);
        self.line("     (local.get $__rel)");
        self.line(&format!("     (call {})", call));
    }

    /// Like [`Self::retain_container_value`] but for a field/element written from an rvalue: a
    /// *borrowed* value (`Use(Copy(place))`) is retained, while an owned producer (call/new/array
    /// literal result) transfers its `+1` into the container and is left as-is.
    fn retain_stored_rvalue(&mut self, ty: TypeId, rvalue: &Rvalue) {
        if let Rvalue::Use(value) = rvalue {
            self.retain_container_value(ty, value);
        }
    }

    /// Writes a zero of `field_ty`'s width into the object under construction (`$__obj + offset`).
    /// Used to clear a struct before a user constructor runs (reused heap blocks are not zeroed).
    pub(super) fn zero_at_obj(&mut self, offset: u32, field_ty: TypeId) {
        self.line("     (local.get $__obj)");
        if offset > 0 {
            self.line(&format!("     (i32.const {})", offset));
            self.line("     (i32.add)");
        }
        let zero = match self.store_instr(field_ty) {
            "f64.store" => "(f64.const 0)",
            "f32.store" => "(f32.const 0)",
            "i64.store" => "(i64.const 0)",
            _ => "(i32.const 0)",
        };
        self.line(&format!("     {}", zero));
        self.line(&format!("     ({})", self.store_instr(field_ty)));
    }
}
