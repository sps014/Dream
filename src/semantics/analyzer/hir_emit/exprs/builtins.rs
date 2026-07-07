//! HIR for compiler-known builtins and runtime shims: `print`, `size`, `is`-tests, string
//! concatenation, enum/object `to_string`/`hash_code`, array/byte allocation, `char_at`, `await`.

use super::*;

impl<'a> Analyzer<'a> {
    /// Records a `print`/`println` builtin call as [`HExprKind::Print`] (void). Every scalar
    /// primitive is covered: `int`/`char`/`string` go straight to a host import, while the other
    /// numerics and `bool` route through the in-wasm `*_to_string` runtime before `$print_string`.
    /// Non-scalar (object/struct/array) arguments still need the object-protocol `to_string` and so
    /// drop the enclosing function out of HIR coverage until that runtime lands.
    pub(in crate::semantics::analyzer) fn hir_set_print(
        &mut self,
        arg: Option<HExpr>,
        newline: bool,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(arg) = arg else {
            self.hir.ok = false;
            self.hir.last = None;
            return;
        };
        let base = self.type_ctx.interner.strip_nullable(arg.ty);
        // Scalars print directly; enums print as their `int` value; every reference type (struct,
        // union, array, `object`) renders through the backend's tag-dispatching `$print_object`.
        let printable = matches!(
            self.type_ctx.interner.kind(base),
            TyKind::Prim(_)
                | TyKind::Enum(_)
                | TyKind::Struct(..)
                | TyKind::Union(..)
                | TyKind::Array(_)
                | TyKind::Object
                | TyKind::Interface(..)
        );
        if !printable {
            self.hir.ok = false;
            self.hir.last = None;
            return;
        }
        let void = self.type_ctx.interner.void();
        self.hir.last = Some(HExpr::new(
            void,
            HExprKind::Print {
                arg: Box::new(arg),
                newline,
            },
        ));
    }

    /// Records `recv.size()` (typed `int`): an array reads its stored length word (`ArrayLen`), while a
    /// string reads its length word via `StrLen` (both are O(1) length-prefixed loads, but they remain
    /// distinct rvalues because the element layouts differ).
    pub(in crate::semantics::analyzer) fn hir_set_array_len(&mut self, recv: Option<HExpr>) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match recv {
            Some(e) => {
                let int = self.type_ctx.interner.int();
                let is_string = matches!(
                    self.type_ctx
                        .interner
                        .kind(self.type_ctx.interner.strip_nullable(e.ty)),
                    TyKind::Prim(PrimTy::String)
                );
                let kind = if is_string {
                    HExprKind::StrLen(Box::new(e))
                } else {
                    HExprKind::ArrayLen(Box::new(e))
                };
                self.hir.last = Some(HExpr::new(int, kind));
            }
            None => self.hir.last = None,
        }
    }

    /// Records a compile-time-known boolean (e.g. the result of a statically-resolved `is` test).
    pub(in crate::semantics::analyzer) fn hir_set_bool(&mut self, value: bool) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let ty = self.type_ctx.interner.bool();
        self.hir.last = Some(HExpr::new(ty, HExprKind::BoolLit(value)));
    }

    /// Records a runtime type test `value is target` (typed `bool`) for an `object`-typed operand:
    /// the backend compares the value's runtime tag against `target`'s. Fails if `value` was dropped.
    pub(in crate::semantics::analyzer) fn hir_set_is_type(
        &mut self,
        value: Option<HExpr>,
        target: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let bool_ty = self.type_ctx.interner.bool();
        let target_ty = self.type_ctx.lower(target);
        self.hir.last = value.map(|v| {
            HExpr::new(
                bool_ty,
                HExprKind::IsType {
                    value: Box::new(v),
                    target: target_ty,
                },
            )
        });
    }

    /// Records string concatenation `a + b` (typed `string`): each non-string operand is first run
    /// through its `to_string` (a C-style enum renders its variant name; everything else uses the
    /// object protocol), then the two string pointers are joined by the runtime `$concat_strings`.
    /// Drops out of coverage if either operand is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_concat(
        &mut self,
        lhs: Option<HExpr>,
        lhs_ty: &crate::syntax::nodes::Type,
        rhs: Option<HExpr>,
        rhs_ty: &crate::syntax::nodes::Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
            self.hir.last = None;
            return;
        };
        let string = self.type_ctx.interner.prim(PrimTy::String);
        let lhs = self.concat_stringify(lhs, lhs_ty, string);
        let rhs = self.concat_stringify(rhs, rhs_ty, string);
        self.hir.last = Some(HExpr::new(
            string,
            HExprKind::Concat(Box::new(lhs), Box::new(rhs)),
        ));
    }

    /// Converts a concatenation operand to a `string`-typed HExpr: string operands pass through, a
    /// C-style enum maps its discriminant to the interned variant name (matching `.to_string()`),
    /// and any other type goes through the object-protocol `to_string`.
    fn concat_stringify(
        &self,
        e: HExpr,
        ty: &crate::syntax::nodes::Type,
        string: crate::types::TypeId,
    ) -> HExpr {
        if ty.is_string() {
            return e;
        }
        let base = crate::syntax::nodes::types::strip_nullable(&ty.get_type()).to_string();
        if let Some(members) = self.enum_table.get(&base) {
            let arms: Vec<(i64, String)> = members
                .iter()
                .map(|(name, value)| (*value as i64, name.clone()))
                .collect();
            return HExpr::new(
                string,
                HExprKind::EnumName {
                    value: Box::new(e),
                    arms,
                },
            );
        }
        HExpr::new(string, HExprKind::ToString(Box::new(e)))
    }

    /// Records a C-style enum's `to_string()` (typed `string`): the backend maps the receiver's
    /// discriminant to its interned variant-name string via `arms` (`(discriminant, name)` for
    /// every member).
    pub(in crate::semantics::analyzer) fn hir_set_enum_name(
        &mut self,
        recv: Option<HExpr>,
        arms: Vec<(i64, String)>,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match recv {
            Some(e) => {
                let string = self.type_ctx.interner.prim(PrimTy::String);
                self.hir.last = Some(HExpr::new(
                    string,
                    HExprKind::EnumName {
                        value: Box::new(e),
                        arms,
                    },
                ));
            }
            None => self.hir.last = None,
        }
    }

    /// Records the object-protocol `x.hash_code()` (typed `int`): the backend dispatches on the
    /// receiver's static type. Drops out of coverage if the receiver is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_hash_code(&mut self, recv: Option<HExpr>) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match recv {
            Some(e) => {
                let int = self.type_ctx.interner.int();
                self.hir.last = Some(HExpr::new(int, HExprKind::HashCode(Box::new(e))));
            }
            None => self.hir.last = None,
        }
    }

    /// Records the object-protocol `x.to_string()` (typed `string`): the backend dispatches on the
    /// receiver's static type. Drops out of coverage if the receiver is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_to_string(&mut self, recv: Option<HExpr>) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match recv {
            Some(e) => {
                let string = self.type_ctx.interner.prim(PrimTy::String);
                self.hir.last = Some(HExpr::new(string, HExprKind::ToString(Box::new(e))));
            }
            None => self.hir.last = None,
        }
    }

    /// Records `Buffer.alloc<T>(len)` (typed `T[]`): a zero-initialized array allocation. Drops out of
    /// coverage if the length is not representable.
    /// Records an empty array literal `[]` of element type `elem_ty` as a zero-length allocation
    /// (equivalent to `Buffer.alloc<T>(0)`).
    pub(in crate::semantics::analyzer) fn hir_set_empty_array(&mut self, elem_ty: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let int = self.type_ctx.interner.int();
        let zero = HExpr::new(int, HExprKind::IntLit(0));
        self.hir_set_array_new(elem_ty, Some(zero));
    }

    pub(in crate::semantics::analyzer) fn hir_set_array_new(
        &mut self,
        elem_ty: &Type,
        len: Option<HExpr>,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match len {
            Some(len) => {
                let elem = self.type_ctx.lower(elem_ty);
                let arr = self.type_ctx.interner.array(elem);
                self.hir.last = Some(HExpr::new(
                    arr,
                    HExprKind::ArrayNew {
                        elem_ty: elem,
                        len: Box::new(len),
                    },
                ));
            }
            None => self.hir.last = None,
        }
    }

    /// Records `Bytes.of<T>(v)` — a raw byte-copy of blittable value `v` into a fresh `byte[]`.
    /// Drops out of coverage if the value is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_to_bytes(&mut self, value: Option<HExpr>) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match value {
            Some(value) => {
                let byte_ty = self.type_ctx.interner.byte();
                let arr = self.type_ctx.interner.array(byte_ty);
                self.hir.last = Some(HExpr::new(arr, HExprKind::ToBytes(Box::new(value))));
            }
            None => self.hir.last = None,
        }
    }

    /// Records `Bytes.to<T>(bytes)` — reconstructs a blittable value of `target` from a `byte[]`
    /// buffer. Drops out of coverage if the buffer operand is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_from_bytes(
        &mut self,
        target: &Type,
        bytes: Option<HExpr>,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match bytes {
            Some(bytes) => {
                let ty = self.type_ctx.lower(target);
                self.hir.last = Some(HExpr::new(ty, HExprKind::FromBytes(Box::new(bytes))));
            }
            None => self.hir.last = None,
        }
    }

    /// Records `recv.char_at(idx)` (typed `char`): a runtime `$char_at` read. Drops out of coverage
    /// if either the receiver or the index is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_char_at(
        &mut self,
        recv: Option<HExpr>,
        idx: Option<HExpr>,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (recv, idx) {
            (Some(r), Some(i)) => {
                let char_ty = self.type_ctx.interner.prim(PrimTy::Char);
                self.hir.last = Some(HExpr::new(
                    char_ty,
                    HExprKind::CharAt(Box::new(r), Box::new(i)),
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records `await e` used as a value (carrying the awaited future's inner type).
    pub(in crate::semantics::analyzer) fn hir_set_await(
        &mut self,
        inner: Option<HExpr>,
        inner_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match inner {
            Some(e) => {
                let ty = self.type_ctx.lower(inner_ty);
                self.hir.last = Some(HExpr::new(ty, HExprKind::Await(Box::new(e))));
            }
            None => self.hir.last = None,
        }
    }
}
