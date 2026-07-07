//! Cast emission and the numeric-conversion helpers it shares with call-argument widening: value
//! struct boxing, primitive box/unbox, struct<->js marshaling dispatch, and the WASM numeric
//! conversion instruction selection. Methods on the parent module's private `Emitter`.

use super::*;

impl Emitter<'_> {
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

    pub(super) fn emit_cast(&mut self, o: &Operand, from: TypeId, to: TypeId) {
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

    /// The common numeric type of a binary operation's operands: the one with the wider WASM value
    /// type, so the narrower side can be widened up to it. Ranking `i32 < i64 < f32 < f64` matches the
    /// language's implicit numeric widening (e.g. `long` op `int` -> `long`; any op `double` ->
    /// `double`). Non-numeric operands (equal-width pointers, `bool`, refs) fall through to `a`, which
    /// leaves same-width pairs unchanged (`emit_numeric_conv` is then a no-op).
    pub(super) fn wider_numeric(&self, a: TypeId, b: TypeId) -> TypeId {
        let rank = |w: &str| match w {
            "i32" => 0,
            "i64" => 1,
            "f32" => 2,
            "f64" => 3,
            _ => -1,
        };
        if rank(self.wasm_ty(b).as_str()) > rank(self.wasm_ty(a).as_str()) {
            b
        } else {
            a
        }
    }

    /// Emits the WASM numeric conversion instruction to turn a value of type `from` (already on the
    /// stack) into type `to`, if their WASM value types differ (a no-op otherwise). Shared by explicit
    /// `Cast` and the implicit widening applied to call arguments.
    pub(super) fn emit_numeric_conv(&mut self, from: TypeId, to: TypeId) {
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
