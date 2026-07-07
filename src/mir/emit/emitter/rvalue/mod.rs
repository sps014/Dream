//! Rvalue (expression) emission for the WAT backend: the big `emit_rvalue` dispatch. The helpers it
//! drives are split by concern into sibling submodules:
//! - [`casts`]: `Cast` lowering (value struct boxing, primitive box/unbox, struct<->js marshaling)
//!   and the WASM numeric-conversion helpers shared with call-argument widening.
//! - [`calls`]: call-argument widening and the interface/indirect call-shape emission (value + sret).
//!
//! All are methods on the parent module's private `Emitter`, so they can reach its other helpers.

use super::*;

mod calls;
mod casts;

impl Emitter<'_> {
    pub(super) fn emit_rvalue(&mut self, rvalue: &Rvalue) {
        match rvalue {
            Rvalue::Use(o) => self.emit_operand(o),
            Rvalue::Select {
                cond,
                then_val,
                else_val,
            } => {
                // WASM `select` pops [val1, val2, cond] and yields val1 when cond != 0.
                self.emit_operand(then_val);
                self.emit_operand(else_val);
                self.emit_operand(cond);
                self.line("     (select)");
            }
            Rvalue::Binary(op, a, b) => {
                let ta = self.operand_ty(a);
                let tb = self.operand_ty(b);
                // String equality compares contents, not pointers, via the runtime `$string_eq`.
                let str_eq = matches!(op, BinOp::Eq | BinOp::Ne)
                    && matches!(
                        self.interner.kind(self.interner.strip_nullable(ta)),
                        TyKind::Prim(PrimTy::String)
                    );
                if str_eq {
                    self.emit_operand(a);
                    self.emit_operand(b);
                    self.line("     (call $string_eq)");
                    if matches!(op, BinOp::Ne) {
                        self.line("     (i32.eqz)");
                    }
                } else {
                    // The operation runs at one WASM width, so widen the narrower operand to the
                    // common numeric type (e.g. `someLong > 0` widens the `int` literal `0` to i64).
                    // Without this a mixed-width pair emits e.g. `i64.gt_s` over an i32 operand,
                    // which fails WASM validation.
                    let common = self.wider_numeric(ta, tb);
                    self.emit_operand(a);
                    self.emit_numeric_conv(ta, common);
                    self.emit_operand(b);
                    self.emit_numeric_conv(tb, common);
                    self.line(&format!("     ({})", self.binop_instr(*op, common)));
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
                        crate::internal_error!(
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
                    crate::internal_error!("missing layout for struct allocation (type {:?})", ty);
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
                            crate::internal_error!(
                                "union def{} variant {} has no field slot for argument {}",
                                def.0,
                                variant,
                                i
                            )
                        });
                        self.store_at_obj(off, fty, arg);
                    }
                    self.line("     (local.get $__obj)");
                } else {
                    // A union that survived analysis always has a registered layout; a miss is a
                    // compiler bug, so trap loudly rather than emitting a null pointer.
                    crate::internal_error!(
                        "missing layout for union def{} variant {}",
                        def.0,
                        variant
                    );
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
                        crate::internal_error!(
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
            Rvalue::ToBytes { value, ty } => {
                // Allocate a `byte[]` of `[len: i32][size bytes]` and raw-copy the value's inline
                // bytes into the payload. `byte` elements are one byte, so the length word is the
                // byte count.
                let size = self.value_size(*ty);
                self.line(&format!("     (i32.const {}) ;; 4 + byte size", 4 + size));
                self.line(&format!("     (i32.const {}) ;; array tag", ARRAY_TAG));
                self.line("     (call $malloc)");
                self.line("     (local.set $__obj)");
                self.line("     (local.get $__obj)");
                self.line(&format!("     (i32.const {})", size));
                self.line("     (i32.store) ;; byte length");
                // memory.copy(dst = obj+4, src = value address, size)
                self.line("     (local.get $__obj)");
                self.line("     (i32.const 4)");
                self.line("     (i32.add)");
                self.emit_operand(value);
                self.line(&format!("     (i32.const {})", size));
                self.line("     (memory.copy)");
                self.line("     (local.get $__obj)");
            }
            Rvalue::FromBytes { bytes, ty } => {
                // Allocate a fresh `T`-sized block (tagged as `T`) and raw-copy the buffer's payload
                // (which starts after the 4-byte length prefix) into it.
                let size = self.value_size(*ty);
                let tag = self.type_tag(*ty, crate::types::DefId(0));
                self.line(&format!("     (i32.const {})", size));
                self.line(&format!("     (i32.const {}) ;; tag", tag));
                self.line("     (call $malloc)");
                self.line("     (local.set $__obj)");
                // memory.copy(dst = obj, src = bytes+4, size)
                self.line("     (local.get $__obj)");
                self.emit_operand(bytes);
                self.line("     (i32.const 4)");
                self.line("     (i32.add)");
                self.line(&format!("     (i32.const {})", size));
                self.line("     (memory.copy)");
                self.line("     (local.get $__obj)");
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
                    crate::internal_error!("`is` target type {:?} has no runtime tag", target)
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
                    crate::internal_error!(
                        "missing layout for union payload (type {:?}, variant {}, field {})",
                        ty,
                        variant,
                        field
                    );
                }
            }
        }
    }

}
