//! Rvalue (expression) emission for the WAT backend: the big `emit_rvalue` dispatch plus the
//! cast, call-argument, interface-call, and numeric-conversion helpers it drives. Split out of
//! `emitter.rs` (which retains the struct, statement, place, and value-struct machinery). These
//! are methods on the parent module's private `Emitter`, so they can reach its other helpers.

use super::*;

impl Emitter<'_> {
    pub(super) fn emit_rvalue(&mut self, rvalue: &Rvalue) {
        match rvalue {
            Rvalue::Use(o) => self.emit_operand(o),
            Rvalue::Binary(op, a, b) => {
                let ty = self.operand_ty(a);
                // String equality compares contents, not pointers, via the runtime `$string_eq`.
                let str_eq = matches!(op, BinOp::Eq | BinOp::Ne)
                    && matches!(
                        self.interner.kind(self.interner.strip_nullable(ty)),
                        TyKind::Prim(PrimTy::String)
                    );
                self.emit_operand(a);
                self.emit_operand(b);
                if str_eq {
                    self.line("     (call $string_eq)");
                    if matches!(op, BinOp::Ne) {
                        self.line("     (i32.eqz)");
                    }
                } else {
                    self.line(&format!("     ({})", self.binop_instr(*op, ty)));
                }
            }
            Rvalue::Unary(op, a) => {
                let ty = self.operand_ty(a);
                match op {
                    UnOp::Neg => {
                        // No `neg` for integers in WASM: 0 - x.
                        if matches!(
                            self.interner.kind(ty),
                            TyKind::Prim(PrimTy::Float | PrimTy::Double)
                        ) {
                            self.emit_operand(a);
                            self.line(&format!("     ({}.neg)", self.wasm_ty(ty)));
                        } else {
                            self.line(&format!("     ({}.const 0)", self.wasm_ty(ty)));
                            self.emit_operand(a);
                            self.line(&format!("     ({}.sub)", self.wasm_ty(ty)));
                        }
                    }
                    UnOp::Not => {
                        self.emit_operand(a);
                        self.line("     (i32.eqz)");
                    }
                }
            }
            Rvalue::Call { callee, args } => {
                let sym = self.callee_symbol(callee);
                if let Some(kind) = async_intrinsic_kind(&sym) {
                    self.emit_async_intrinsic(kind, args);
                } else {
                    self.emit_call_args(callee, args);
                    self.line(&format!("     (call ${sym})"));
                }
            }
            Rvalue::IndirectCall { target, args } => {
                for a in args {
                    self.emit_operand(a);
                }
                self.emit_operand(target);
                // The table index (target) is on top of the stack; dispatch through `$__ft` with the
                // signature derived from the target's function type.
                let sig = func_sig(self.interner, self.operand_ty(target))
                    .map(|(name, _, _)| name)
                    .unwrap_or_else(|| "$sig___v".to_string());
                self.line(&format!("     (call_indirect $__ft (type {}))", sig));
            }
            Rvalue::InterfaceCall {
                receiver,
                iface_id,
                method_slot,
                sig,
                args,
                ..
            } => {
                self.emit_interface_call(receiver, *iface_id, *method_slot, *sig, args);
            }
            Rvalue::JsCall {
                callee,
                target,
                method,
                args,
            } => {
                self.emit_js_call(callee, target, method.as_ref(), args);
            }
            Rvalue::FuncRef(callee) => {
                // A function value is its slot index in the module function table. The table is
                // built from every referenced function, so a miss means it diverged from MIR
                // (compiler bug); trap loudly rather than silently referencing slot 0.
                let idx = self
                    .func_table
                    .get(&(callee.def, callee.args.clone()))
                    .copied()
                    .unwrap_or_else(|| {
                        unreachable!(
                            "funcref to def{} missing from the function table",
                            callee.def.0
                        )
                    });
                self.line(&format!(
                    "     (i32.const {}) ;; funcref def{}",
                    idx, callee.def.0
                ));
            }
            Rvalue::New {
                def,
                ty,
                ctor,
                args,
            } => {
                // `$malloc(data_size, tag)` returns a data pointer with refcount 1.
                let info = self.layouts.get(*ty).map(|l| {
                    (
                        l.size,
                        l.fields
                            .iter()
                            .map(|f| (f.offset, f.ty))
                            .collect::<Vec<_>>(),
                    )
                });
                if let Some((size, fields)) = info {
                    self.line(&format!("     (i32.const {})", size));
                    self.line(&format!(
                        "     (i32.const {}) ;; tag",
                        self.type_tag(*ty, *def)
                    ));
                    self.line("     (call $malloc)");
                    self.line("     (local.set $__obj)");
                    if let Some(ctor) = ctor {
                        // A user `constructor(this, args...)` sets the fields itself. Reused heap
                        // blocks are not zeroed, so zero every field first (a constructor that leaves a
                        // field unset must observe 0/null), then call it; the object is the result.
                        for &(off, fty) in &fields {
                            self.zero_at_obj(off, fty);
                        }
                        self.line("     (local.get $__obj)");
                        for arg in args {
                            self.emit_operand(arg);
                        }
                        let sym = self.callee_symbol(&crate::mir::Callee {
                            def: *ctor,
                            args: vec![],
                            ret: self.interner.void(),
                        });
                        self.line(&format!("     (call ${})", sym));
                        self.line("     (local.get $__obj)");
                    } else {
                        // Implicit zero-arg default constructor: leave every field at its zero
                        // value. Reused heap blocks are not zeroed, so zero each field explicitly.
                        let _ = args;
                        for &(off, fty) in &fields {
                            self.zero_at_obj(off, fty);
                        }
                        self.line("     (local.get $__obj)");
                    }
                } else {
                    unreachable!("Missing layout for struct allocation");
                }
            }
            Rvalue::UnionNew {
                def,
                ty,
                variant,
                args,
            } => {
                // A union value is one heap block `[discriminant: i32][payload...]`, sized to the
                // largest variant so any variant fits. `variant` is the discriminant; allocate,
                // write it at offset 0, then store the payload at the variant's field offsets.
                let layout = self.layouts.union(*ty).and_then(|u| {
                    let size = u.size;
                    u.variants
                        .iter()
                        .find(|v| v.discriminant as usize == *variant)
                        .map(|v| {
                            (
                                size,
                                v.fields
                                    .iter()
                                    .map(|f| (f.offset, f.ty))
                                    .collect::<Vec<_>>(),
                            )
                        })
                });
                if let Some((size, fields)) = layout {
                    // The analyzer already checked the variant's arity, so the argument list and the
                    // variant's field slots must line up; a mismatch would silently drop or
                    // misplace payload words.
                    debug_assert_eq!(
                        args.len(),
                        fields.len(),
                        "union def{} variant {} arity ({} args) disagrees with its layout ({} fields)",
                        def.0, variant, args.len(), fields.len()
                    );
                    self.line(&format!("     (i32.const {})", size));
                    self.line(&format!(
                        "     (i32.const {}) ;; tag",
                        self.type_tag(*ty, *def)
                    ));
                    self.line("     (call $malloc)");
                    self.line("     (local.set $__obj)");
                    self.line("     (local.get $__obj)");
                    self.line(&format!("     (i32.const {}) ;; discriminant", variant));
                    self.line("     (i32.store)");
                    for (i, arg) in args.iter().enumerate() {
                        let &(off, fty) = fields.get(i).unwrap_or_else(|| {
                            unreachable!(
                                "union def{} variant {} has no field slot for argument {}",
                                def.0, variant, i
                            )
                        });
                        self.store_at_obj(off, fty, arg);
                    }
                    self.line("     (local.get $__obj)");
                } else {
                    // A union that survived analysis always has a registered layout; a miss is a
                    // compiler bug, so trap loudly rather than emitting a null pointer.
                    unreachable!("Missing layout for union def{} variant {}", def.0, variant);
                }
            }
            Rvalue::ArrayLit { elem_ty, elems } => {
                // Array block: `[len: i32][elem0][elem1]...`; the length is the first word (matching
                // `ArrayLen`), elements follow at stride `elem_size`.
                let (esize, _) = scalar_size(self.interner, *elem_ty);
                // `[len:i32] + count * esize`. A literal big enough to overflow u32 is not
                // representable in source, but guard the arithmetic so a bug can never emit a
                // silently-truncated (undersized) allocation.
                let size = (elems.len() as u32)
                    .checked_mul(esize)
                    .and_then(|payload| payload.checked_add(4))
                    .unwrap_or_else(|| {
                        unreachable!(
                            "array literal size overflows u32 ({} elems x {} bytes)",
                            elems.len(),
                            esize
                        )
                    });
                self.line(&format!("     (i32.const {})", size));
                self.line(&format!("     (i32.const {}) ;; array tag", ARRAY_TAG));
                self.line("     (call $malloc)");
                self.line("     (local.set $__obj)");
                self.line("     (local.get $__obj)");
                self.line(&format!("     (i32.const {})", elems.len()));
                self.line("     (i32.store) ;; length");
                for (i, e) in elems.iter().enumerate() {
                    self.store_at_obj(4 + esize * (i as u32), *elem_ty, e);
                }
                self.line("     (local.get $__obj)");
            }
            Rvalue::ArrayNew { elem_ty, len } => {
                // Block: `[len: i32][elem0..]`, zero-initialized (recycled freelist blocks are not
                // zeroed, and reference-typed releases rely on null slots).
                let (esize, _) = scalar_size(self.interner, *elem_ty);
                self.emit_operand(len);
                self.line("     (local.set $__len)");
                // size = 4 + len * esize
                self.line("     (i32.const 4)");
                self.line("     (local.get $__len)");
                self.line(&format!("     (i32.const {})", esize));
                self.line("     (i32.mul)");
                self.line("     (i32.add)");
                self.line(&format!("     (i32.const {}) ;; array tag", ARRAY_TAG));
                self.line("     (call $malloc)");
                self.line("     (local.set $__obj)");
                self.line("     (local.get $__obj)");
                self.line("     (local.get $__len)");
                self.line("     (i32.store) ;; length");
                // memory.fill(dst = obj+4, 0, len*esize)
                self.line("     (local.get $__obj)");
                self.line("     (i32.const 4)");
                self.line("     (i32.add)");
                self.line("     (i32.const 0)");
                self.line("     (local.get $__len)");
                self.line(&format!("     (i32.const {})", esize));
                self.line("     (i32.mul)");
                self.line("     (memory.fill)");
                self.line("     (local.get $__obj)");
            }
            Rvalue::ArrayLen(o) => {
                self.emit_operand(o);
                self.line("     (i32.load) ;; array length is the first word");
            }
            Rvalue::CharAt(s, i) => {
                self.emit_operand(s);
                self.emit_operand(i);
                self.line("     (call $char_at)");
            }
            Rvalue::Concat(a, b) => {
                self.emit_operand(a);
                self.emit_operand(b);
                self.line("     (call $concat_strings)");
            }
            Rvalue::ToString(o) => {
                self.emit_operand(o);
                let oty = self.operand_ty(o);
                // A value struct/union is addressed inline (no heap tag header), so its `to_string`
                // is dispatched statically to the concrete `$<Type>_to_string` rather than routed
                // through the tag-dispatching `$object_to_string`.
                if self.interner.is_value_type(oty) {
                    if let Some(name) = self.value_name(oty) {
                        self.line(&format!("     (call ${}_to_string)", name));
                        return;
                    }
                }
                // A `string` is already its own `to_string`; every other type has a formatter.
                if let Some(call) = value_to_string_call(self.interner, oty) {
                    self.line(&format!("     (call {})", call));
                }
            }
            Rvalue::EnumName { value, arms } => {
                let empty = self.string_addr("");
                self.emit_operand(value);
                self.line("     (local.set $__len)");
                // Nested `value == disc ? strptr : (...)`, terminating in the empty string.
                for (disc, name) in arms {
                    let ptr = self.string_addr(name);
                    self.line("     (local.get $__len)");
                    self.line(&format!("     (i32.const {})", disc));
                    self.line("     (i32.eq)");
                    self.line("     (if (result i32)");
                    self.line(&format!("      (then (i32.const {}))", ptr));
                    self.line("      (else");
                }
                self.line(&format!("     (i32.const {})", empty));
                for _ in arms {
                    self.line("     ))");
                }
            }
            Rvalue::HashCode(o) => {
                self.emit_operand(o);
                let oty = self.operand_ty(o);
                if self.interner.is_value_type(oty) {
                    if let Some(name) = self.value_name(oty) {
                        self.line(&format!("     (call ${}_hash_code)", name));
                        return;
                    }
                }
                match self.interner.kind(self.interner.strip_nullable(oty)) {
                    // Integer-family values (and enums) are their own hash.
                    TyKind::Prim(
                        PrimTy::Int | PrimTy::UInt | PrimTy::Bool | PrimTy::Char | PrimTy::Byte,
                    )
                    | TyKind::Enum(_) => {}
                    TyKind::Prim(PrimTy::Long | PrimTy::ULong) => {
                        self.line("     (call $hash_long)")
                    }
                    TyKind::Prim(PrimTy::Float) => self.line("     (i32.reinterpret_f32)"),
                    TyKind::Prim(PrimTy::Double) => self.line("     (call $hash_double)"),
                    TyKind::Prim(PrimTy::String) => self.line("     (call $hash_string)"),
                    _ => self.line("     (call $object_hash_code)"),
                }
            }
            Rvalue::StrLen(o) => {
                self.emit_operand(o);
                self.line(
                    "     (call $strlen) ;; O(1): length is stored at the string's data pointer",
                );
            }
            Rvalue::Cast(o, from, to) => self.emit_cast(o, *from, *to),
            Rvalue::IsType(o, target) => {
                self.emit_operand(o);
                self.line("     (call $object_tag)");
                // The analyzer only admits `is` against a type with a concrete runtime tag; a
                // `None` here means an unsupported target slipped through (compiler bug). Comparing
                // against 0 would silently answer the wrong question, so fail loudly instead.
                let tag = runtime_tag_for(self.interner, self.tags, *target).unwrap_or_else(|| {
                    unreachable!("`is` target type {:?} has no runtime tag", target)
                });
                self.line(&format!("     (i32.const {})", tag));
                self.line("     (i32.eq)");
            }
            Rvalue::Discriminant(o) => {
                // The discriminant is the `i32` at offset 0 of the union block.
                self.emit_operand(o);
                self.line("     (i32.load) ;; union discriminant");
            }
            Rvalue::UnionField {
                base,
                ty,
                variant,
                field,
            } => {
                let slot = self.layouts.union(*ty).and_then(|u| {
                    u.variants
                        .iter()
                        .find(|v| v.discriminant as usize == *variant)
                        .and_then(|v| v.fields.get(*field))
                        .map(|f| (f.offset, f.ty))
                });
                if let Some((off, fty)) = slot {
                    self.emit_operand(base);
                    if off > 0 {
                        self.line(&format!("     (i32.const {})", off));
                        self.line("     (i32.add)");
                    }
                    // A value-struct payload is addressed inline (its bytes live in the union block),
                    // so reading it yields the payload address rather than a load.
                    if !self.interner.is_value_type(fty) {
                        self.line(&format!("     ({})", self.load_instr(fty)));
                    }
                } else {
                    unreachable!("Missing layout for union payload");
                }
            }
        }
    }

    /// Boxes a value struct `ty` (whose operand pushes its inline address) into a fresh tagged heap
    /// object: `$malloc(size, tag)`, `memory.copy` the inline bytes in, then retain the copy's
    /// embedded references. Leaves the heap data pointer on the stack (refcount 1, owned).
    fn emit_box_value_struct(&mut self, o: &Operand, ty: TypeId) {
        let size = self.value_size(ty);
        let tag = self.type_tag(ty, crate::types::DefId(0));
        self.line(&format!("     (i32.const {})", size));
        self.line(&format!("     (i32.const {}) ;; tag", tag));
        self.line("     (call $malloc)");
        self.line("     (local.set $__obj)");
        // memory.copy(dst = $__obj, src = inline address of the value, size)
        self.line("     (local.get $__obj)");
        self.emit_operand(o);
        self.line(&format!("     (i32.const {})", size));
        self.line("     (memory.copy)");
        if self.value_has_glue(ty) {
            if let Some(name) = self.value_name(ty) {
                self.line(&format!(
                    "     (local.get $__obj) (call {})",
                    vs_retain_sym(&name)
                ));
            }
        }
        self.line("     (local.get $__obj)");
    }

    fn emit_cast(&mut self, o: &Operand, from: TypeId, to: TypeId) {
        // A struct/class <-> `js` cast routes through the generated deep-copy marshalers (see
        // `js_marshal`); everything else falls through to the primitive box/unbox path below.
        if let Some(sym) = js_marshal::cast_sym(self.interner, self.layouts, from, to) {
            self.emit_operand(o);
            self.line(&format!("     (call {})", sym));
            return;
        }
        // Boxing a value struct into a reference target (`object`, an interface, or a nullable value
        // struct `T?`): allocate a tagged heap block, byte-copy the inline value in, and retain the
        // copy's embedded references. The result is a refcounted heap object indistinguishable from
        // a class instance for dynamic dispatch, the object protocol, and deep release.
        let to_ref = self.interner.strip_nullable(to);
        let from_bare = self.interner.strip_nullable(from);
        let to_is_ref_box = matches!(
            self.interner.kind(to_ref),
            TyKind::Object | TyKind::Interface(..)
        ) || self.interner.is_nullable_boxed_value(to);
        if to_is_ref_box
            && self.interner.is_value_type(from_bare)
            && !self.interner.is_nullable_boxed_value(from)
        {
            self.emit_box_value_struct(o, from_bare);
            return;
        }
        let from_prim = prim_of(self.interner, from);
        let to_prim = prim_of(self.interner, to);
        let to_is_object = matches!(
            self.interner.kind(self.interner.strip_nullable(to)),
            TyKind::Object
        );
        let from_is_object = matches!(
            self.interner.kind(self.interner.strip_nullable(from)),
            TyKind::Object
        );
        // Boxing a primitive into `object` (reference types are already pointers → identity).
        if to_is_object {
            self.emit_operand(o);
            if let Some(boxfn) = from_prim.and_then(box_fn_for) {
                self.line(&format!("     (call {})", boxfn));
            }
            return;
        }
        // Unboxing `object` to a primitive (or leaving a reference pointer as-is).
        if from_is_object {
            self.emit_operand(o);
            if let Some(unboxfn) = to_prim.and_then(unbox_fn_for) {
                self.line(&format!("     (call {})", unboxfn));
            }
            return;
        }
        self.emit_operand(o);
        self.emit_numeric_conv(from, to);
        // Narrowing to `byte` (which shares the `i32` WASM type with `int`/`uint`, so `numeric_conv`
        // is a no-op) must wrap into the [0, 255] range explicitly (C-style truncation).
        if matches!(to_prim, Some(PrimTy::Byte)) {
            self.line("     (i32.const 255)");
            self.line("     (i32.and)");
        }
    }

    /// Emits a call's arguments, applying implicit numeric widening to each so a narrower argument
    /// (e.g. an `int`/`float` passed to a `double` parameter) matches the callee's WASM signature.
    /// Falls back to a plain push when the callee's parameter types are unknown (imports/intrinsics).
    pub(super) fn emit_call_args(&mut self, callee: &crate::mir::Callee, args: &[Operand]) {
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
    pub(super) fn emit_interface_call(
        &mut self,
        receiver: &Operand,
        iface_id: usize,
        method_slot: usize,
        sig: TypeId,
        args: &[Operand],
    ) {
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
    pub(super) fn emit_interface_sret_call(
        &mut self,
        dst: impl Fn(&mut Self),
        receiver: &Operand,
        iface_id: usize,
        method_slot: usize,
        sig: TypeId,
        args: &[Operand],
    ) {
        let param_tys: Vec<TypeId> = match self.interner.kind(sig) {
            TyKind::Func(params, _) => params.clone(),
            _ => Vec::new(),
        };
        dst(self);
        self.emit_operand(receiver);
        for (i, a) in args.iter().enumerate() {
            self.emit_operand(a);
            // param_tys[0] is the receiver (`this`); real args start at index 1.
            if let Some(pty) = param_tys.get(i + 1) {
                self.emit_numeric_conv(self.operand_ty(a), *pty);
            }
        }
        self.line(&format!(
            "     (call ${})",
            iface_dispatch_symbol(iface_id, method_slot)
        ));
    }

    /// Emits the WASM numeric conversion instruction to turn a value of type `from` (already on the
    /// stack) into type `to`, if their WASM value types differ (a no-op otherwise). Shared by explicit
    /// `Cast` and the implicit widening applied to call arguments.
    fn emit_numeric_conv(&mut self, from: TypeId, to: TypeId) {
        let (fw, tw) = (self.wasm_ty(from), self.wasm_ty(to));
        if fw != tw {
            // Numeric conversions between the four WASM value types. Integer/float conversions carry
            // the signedness of the *integer* side (the target for float→int, the source otherwise);
            // saturating float→int truncation matches C-style cast semantics (no trap on overflow/NaN).
            let (fw, tw) = (fw.as_str(), tw.as_str());
            let int_signed = |ty: TypeId| {
                !matches!(
                    self.interner.kind(self.interner.strip_nullable(ty)),
                    TyKind::Prim(PrimTy::UInt | PrimTy::ULong | PrimTy::Byte)
                )
            };
            let instr = match (fw, tw) {
                ("i32", "i64") => {
                    if int_signed(from) {
                        "i64.extend_i32_s"
                    } else {
                        "i64.extend_i32_u"
                    }
                }
                ("i64", "i32") => "i32.wrap_i64",
                ("i32", "f32") => {
                    if int_signed(from) {
                        "f32.convert_i32_s"
                    } else {
                        "f32.convert_i32_u"
                    }
                }
                ("i32", "f64") => {
                    if int_signed(from) {
                        "f64.convert_i32_s"
                    } else {
                        "f64.convert_i32_u"
                    }
                }
                ("i64", "f32") => {
                    if int_signed(from) {
                        "f32.convert_i64_s"
                    } else {
                        "f32.convert_i64_u"
                    }
                }
                ("i64", "f64") => {
                    if int_signed(from) {
                        "f64.convert_i64_s"
                    } else {
                        "f64.convert_i64_u"
                    }
                }
                ("f32", "f64") => "f64.promote_f32",
                ("f64", "f32") => "f32.demote_f64",
                ("f32", "i32") => {
                    if int_signed(to) {
                        "i32.trunc_sat_f32_s"
                    } else {
                        "i32.trunc_sat_f32_u"
                    }
                }
                ("f64", "i32") => {
                    if int_signed(to) {
                        "i32.trunc_sat_f64_s"
                    } else {
                        "i32.trunc_sat_f64_u"
                    }
                }
                ("f32", "i64") => {
                    if int_signed(to) {
                        "i64.trunc_sat_f32_s"
                    } else {
                        "i64.trunc_sat_f32_u"
                    }
                }
                ("f64", "i64") => {
                    if int_signed(to) {
                        "i64.trunc_sat_f64_s"
                    } else {
                        "i64.trunc_sat_f64_u"
                    }
                }
                _ => "nop",
            };
            self.line(&format!("     ({})", instr));
        }
    }
}
