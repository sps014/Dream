use super::*;

impl<'a> Analyzer<'a> {
    /// Records the HIR for a literal expression.
    pub(in crate::semantics::analyzer) fn hir_set_literal(&mut self, lit: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let kind = match lit {
            Type::Integer(t) | Type::Long(t) | Type::UInt(t) | Type::ULong(t) | Type::Byte(t) => {
                t.text.parse::<i64>().ok().map(HExprKind::IntLit)
            }
            Type::Float(t) | Type::Double(t) => t.text.parse::<f64>().ok().map(HExprKind::FloatLit),
            Type::Boolean(t) => Some(HExprKind::BoolLit(t.text == "true")),
            // The parser normalizes a char literal's token text to its decimal code point (e.g.
            // `'A'` → "65"), so recover the `char` from that integer rather than the raw glyph.
            Type::Char(t) => t
                .text
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .map(HExprKind::CharLit),
            Type::String(t) => Some(HExprKind::StringLit(string_lit_value(&t.text))),
            // The parser models the bare `null` literal as `Nullable(Void)` until its type is known.
            Type::Nullable(inner) if matches!(**inner, Type::Void) => Some(HExprKind::Null),
            _ => None,
        };
        let mut ty = self.type_ctx.lower(lit);
        // An `int`-typed literal whose value doesn't fit in `i32` is really a `long`: promote its HIR
        // type so the backend emits `i64.const` instead of an out-of-range `i32.const`. (The parser
        // types decimal integer literals as `int` regardless of magnitude.)
        if let Some(HExprKind::IntLit(v)) = &kind {
            if matches!(lit, Type::Integer(_)) && (*v > i32::MAX as i64 || *v < i32::MIN as i64) {
                ty = self.type_ctx.interner.long();
            }
        }
        self.hir.last = kind.map(|k| HExpr::new(ty, k));
    }

    /// Records the HIR for an identifier read: a local-variable reference if the name resolves to a
    /// slot, otherwise `None` (globals and function values are later slices).
    pub(in crate::semantics::analyzer) fn hir_set_var(&mut self, name: &str) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        if let Some(&(local, ty)) = self.hir.locals.get(name) {
            self.hir.last = Some(HExpr::new(ty, HExprKind::Var(Binding::Local(local))));
        } else if let Some(&(global, ty)) = self.hir.globals.get(name) {
            self.hir.last = Some(HExpr::new(ty, HExprKind::Var(Binding::Global(global))));
        } else {
            self.hir.last = None;
        }
    }

    /// Records the HIR for a binary expression from its already-collected operands.
    pub(in crate::semantics::analyzer) fn hir_set_binary(
        &mut self,
        lhs: Option<HExpr>,
        opr: &SyntaxToken,
        rhs: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (token_to_binop(opr.kind), lhs, rhs) {
            (Some(op), Some(lhs), Some(rhs)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for a unary expression. Unary `+` is the identity (passes the operand
    /// through); `-` and `!` map to [`UnOp::Neg`]/[`UnOp::Not`].
    pub(in crate::semantics::analyzer) fn hir_set_unary(&mut self, opr: &SyntaxToken, operand: Option<HExpr>, result_ty: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let op = match opr.kind {
            TokenKind::PlusToken => {
                self.hir.last = operand;
                return;
            }
            TokenKind::MinusToken => UnOp::Neg,
            TokenKind::BangToken => UnOp::Not,
            _ => {
                self.hir.last = None;
                return;
            }
        };
        self.hir.last = operand.map(|operand| {
            let ty = self.type_ctx.lower(result_ty);
            HExpr::new(
                ty,
                HExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
            )
        });
    }

    /// Records the HIR for a `cond ? then : else_` from its already-collected parts.
    pub(in crate::semantics::analyzer) fn hir_set_ternary(
        &mut self,
        cond: Option<HExpr>,
        then_e: Option<HExpr>,
        else_e: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (cond, then_e, else_e) {
            (Some(cond), Some(then_expr), Some(else_expr)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Ternary {
                        cond: Box::new(cond),
                        then_expr: Box::new(then_expr),
                        else_expr: Box::new(else_expr),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for null-coalescing `lhs ?? rhs`.
    pub(in crate::semantics::analyzer) fn hir_set_coalesce(
        &mut self,
        lhs: Option<HExpr>,
        rhs: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (lhs, rhs) {
            (Some(lhs), Some(rhs)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Coalesce {
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for `array[index]` (read position).
    pub(in crate::semantics::analyzer) fn hir_set_index(
        &mut self,
        array: Option<HExpr>,
        index: Option<HExpr>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match (array, index) {
            (Some(array), Some(index)) => {
                let ty = self.type_ctx.lower(result_ty);
                self.hir.last = Some(HExpr::new(
                    ty,
                    HExprKind::Index {
                        array: Box::new(array),
                        index: Box::new(index),
                    },
                ));
            }
            _ => self.hir.last = None,
        }
    }

    /// Records the HIR for a cast `expr as T`; `target_ty` is the cast's result type.
    pub(in crate::semantics::analyzer) fn hir_set_cast(&mut self, inner: Option<HExpr>, target_ty: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        self.hir.last = inner.map(|inner| {
            let ty = self.type_ctx.lower(target_ty);
            HExpr::new(ty, HExprKind::Cast(Box::new(inner)))
        });
    }

    /// Records the HIR for a non-empty array literal. `result_ty` is the array type (`T[]`); the
    /// element type is taken from it. Fails if any element was not representable.
    pub(in crate::semantics::analyzer) fn hir_set_array_lit(&mut self, elems: Vec<Option<HExpr>>, result_ty: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let elem_ty = match result_ty {
            Type::Array(inner) => self.type_ctx.lower(inner),
            _ => {
                self.hir.last = None;
                return;
            }
        };
        let mut collected = Vec::with_capacity(elems.len());
        for e in elems {
            match e {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ty = self.type_ctx.lower(result_ty);
        self.hir.last = Some(HExpr::new(
            ty,
            HExprKind::ArrayLit {
                elem_ty,
                elems: collected,
            },
        ));
    }

    /// Records the HIR for a direct free-function call `name(args)`. Resolves `name` to its function
    /// `DefId`; if it is not a registered (non-generic, non-overloaded) function or any argument is
    /// not representable, the call is dropped from coverage (the function falls back to the legacy
    /// path).
    pub(in crate::semantics::analyzer) fn hir_set_call(&mut self, name: &str, args: Vec<Option<HExpr>>, ret: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, name) else {
            self.hir.last = None;
            return;
        };
        let mut collected = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ret_ty = self.type_ctx.lower(ret);
        let callee = Callee {
            def,
            instance: vec![],
            ret: ret_ty,
        };
        self.hir.last = Some(HExpr::new(
            ret_ty,
            HExprKind::Call {
                callee,
                args: collected,
            },
        ));
    }

    /// Records a first-class function value: a bare function name used as a value (e.g. `let f = foo;`
    /// or passing `foo` to a `fun(...)` parameter) becomes a `Binding::Func` carrying the resolved def
    /// and signature, typed as the function type so the backend can materialize its table index. Drops
    /// coverage if the name is not a registered function def.
    pub(in crate::semantics::analyzer) fn hir_set_func_value(&mut self, name: &str, func_ty: &Type, ret: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, name) else {
            self.hir.last = None;
            return;
        };
        let tid = self.type_ctx.lower(func_ty);
        let ret_ty = self.type_ctx.lower(ret);
        self.hir.last = Some(HExpr::new(
            tid,
            HExprKind::Var(Binding::Func(Callee { def, instance: vec![], ret: ret_ty })),
        ));
    }

    /// Like [`hir_set_func_value`], but for a *generic* function used as a value: the target is the
    /// base template's shared `DefId` plus the concrete `instance` type-args (in binding order), so it
    /// resolves to the same function-table slot the monomorphized instance body emits. Drops coverage
    /// if the base name is unregistered.
    pub(in crate::semantics::analyzer) fn hir_set_generic_func_value(
        &mut self,
        base_name: &str,
        instance: Vec<TypeId>,
        func_ty: &Type,
        ret: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, base_name) else {
            self.hir.last = None;
            return;
        };
        let tid = self.type_ctx.lower(func_ty);
        let ret_ty = self.type_ctx.lower(ret);
        self.hir.last = Some(HExpr::new(
            tid,
            HExprKind::Var(Binding::Func(Callee { def, instance, ret: ret_ty })),
        ));
    }

    /// Records an indirect call `f(args)` where `f` is a function-typed local: the target reads the
    /// local (whose value is a function-table index) and the call dispatches through it. Drops coverage
    /// if the name is not a known local or any argument is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_indirect_call(&mut self, name: &str, args: Vec<Option<HExpr>>, ret: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(&(local, ty)) = self.hir.locals.get(name) else {
            self.hir.last = None;
            return;
        };
        let mut collected = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ret_ty = self.type_ctx.lower(ret);
        let target = HExpr::new(ty, HExprKind::Var(Binding::Local(local)));
        self.hir.last = Some(HExpr::new(
            ret_ty,
            HExprKind::IndirectCall { target: Box::new(target), args: collected },
        ));
    }

    /// Records the HIR for a resolved call to a generic free function. `base_name` is the template's
    /// (unmangled) name — the `DefId` shared by every instance — and `instance` is the concrete
    /// type-args (in binding order) that select the monomorphization. The backend combines
    /// `(def, instance)` into the same symbol the instance body emits. Drops out of coverage if the
    /// base name is unregistered or any argument is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_generic_call(
        &mut self,
        base_name: &str,
        instance: Vec<TypeId>,
        args: Vec<Option<HExpr>>,
        ret: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, base_name) else {
            self.hir.last = None;
            return;
        };
        let mut collected = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ret_ty = self.type_ctx.lower(ret);
        let callee = Callee {
            def,
            instance,
            ret: ret_ty,
        };
        self.hir.last = Some(HExpr::new(
            ret_ty,
            HExprKind::Call {
                callee,
                args: collected,
            },
        ));
    }

    /// Records the HIR for an enum-member reference (`Enum.Member`) resolved to its integer value.
    pub(in crate::semantics::analyzer) fn hir_set_enum_value(&mut self, value: i64, enum_ty: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let ty = self.type_ctx.lower(enum_ty);
        self.hir.last = Some(HExpr::new(ty, HExprKind::EnumValue(value)));
    }

    /// Records the HIR for a struct field read `obj.field`; `field` is the resolved field index
    /// (offset order). Fails over to the legacy path if the receiver was not representable.
    pub(in crate::semantics::analyzer) fn hir_set_field(&mut self, obj: Option<HExpr>, field: usize, field_ty: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        self.hir.last = obj.map(|obj| {
            let ty = self.type_ctx.lower(field_ty);
            HExpr::new(
                ty,
                HExprKind::Field {
                    obj: Box::new(obj),
                    field,
                },
            )
        });
    }

    /// Records the HIR for a constructor call `Struct(args)`. `name` is the source (base) struct name
    /// — the registered `DefId` for both plain and generic structs — and `result_ty` supplies the
    /// per-instance layout key. `ctor`, when `Some`, is the resolved user `constructor(){}` def (its
    /// `args` are the constructor's arguments); when `None`, the implicit zero-arg default
    /// constructor takes no args and every field is zero-initialized.
    /// Unresolved names or a non-representable argument drop the call out of coverage.
    pub(in crate::semantics::analyzer) fn hir_set_new(
        &mut self,
        name: &str,
        ctor: Option<DefId>,
        args: Vec<Option<HExpr>>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(def) = self.type_ctx.defs.lookup(DefKind::Struct, name) else {
            self.hir.last = None;
            return;
        };
        let mut collected = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ty = self.type_ctx.lower(result_ty);
        self.hir.last = Some(HExpr::new(
            ty,
            HExprKind::New {
                def,
                instance: vec![],
                ctor,
                args: collected,
            },
        ));
    }

    /// Records a resolved instance method call `receiver.method(args)`. `mangled` is the registered
    /// `{Type}_{method}` name; if it does not resolve to a `DefId`, or the receiver/any argument is
    /// not representable, the call drops out of coverage.
    pub(in crate::semantics::analyzer) fn hir_set_method_call(
        &mut self,
        receiver: Option<HExpr>,
        mangled: &str,
        args: Vec<Option<HExpr>>,
        ret: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let (Some(def), Some(receiver)) = (
            self.type_ctx.defs.lookup(DefKind::Function, mangled),
            receiver,
        ) else {
            self.hir.last = None;
            return;
        };
        let mut collected = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ret_ty = self.type_ctx.lower(ret);
        let callee = Callee {
            def,
            instance: vec![],
            ret: ret_ty,
        };
        self.hir.last = Some(HExpr::new(
            ret_ty,
            HExprKind::MethodCall {
                receiver: Box::new(receiver),
                callee,
                args: collected,
            },
        ));
    }

    /// Wraps the last-emitted expression in a logical negation (`!expr`), preserving its type. Used
    /// to lower `a != b` after it has been rewritten to the `equals` call `a.equals(b)`.
    pub(in crate::semantics::analyzer) fn hir_negate_last(&mut self) {
        if let Some(expr) = self.hir.last.take() {
            let ty = expr.ty;
            self.hir.last = Some(HExpr::new(
                ty,
                HExprKind::Unary {
                    op: crate::hir::UnOp::Not,
                    operand: Box::new(expr),
                },
            ));
        }
    }

    /// Records a dynamically-dispatched interface method call. `iface` is the interface's `DefId`
    /// and `method_slot` the method's local index within the interface; the backend uses the
    /// receiver's runtime tag to select the concrete implementation. Drops out of coverage if the
    /// receiver or any argument is not representable.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::semantics::analyzer) fn hir_set_interface_call(
        &mut self,
        receiver: Option<HExpr>,
        iface_id: usize,
        method_slot: usize,
        sig: TypeId,
        args: Vec<Option<HExpr>>,
        ret: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(receiver) = receiver else {
            self.hir.last = None;
            return;
        };
        let mut collected = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ret_ty = self.type_ctx.lower(ret);
        self.hir.last = Some(HExpr::new(
            ret_ty,
            HExprKind::InterfaceCall {
                receiver: Box::new(receiver),
                iface_id,
                method_slot,
                sig,
                args: collected,
            },
        ));
    }

    /// Records a discriminated-union construction `Enum.Variant(args)`. `def` is the union's `DefId`
    /// and `variant` its discriminant; any non-representable argument drops it out of coverage.
    pub(in crate::semantics::analyzer) fn hir_set_union_new(
        &mut self,
        def: DefId,
        variant: usize,
        args: Vec<Option<HExpr>>,
        result_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let mut collected = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Some(e) => collected.push(e),
                None => {
                    self.hir.last = None;
                    return;
                }
            }
        }
        let ty = self.type_ctx.lower(result_ty);
        self.hir.last = Some(HExpr::new(
            ty,
            HExprKind::UnionNew {
                def,
                variant,
                args: collected,
            },
        ));
    }
    /// Records a `print`/`println` builtin call as [`HExprKind::Print`] (void). Every scalar
    /// primitive is covered: `int`/`char`/`string` go straight to a host import, while the other
    /// numerics and `bool` route through the in-wasm `*_to_string` runtime before `$print_string`.
    /// Non-scalar (object/struct/array) arguments still need the object-protocol `to_string` and so
    /// drop the enclosing function out of HIR coverage until that runtime lands.
    pub(in crate::semantics::analyzer) fn hir_set_print(&mut self, arg: Option<HExpr>, newline: bool) {
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
            TyKind::Prim(_) | TyKind::Enum(_) | TyKind::Struct(..) | TyKind::Union(..) | TyKind::Array(_) | TyKind::Object | TyKind::Interface(..)
        );
        if !printable {
            self.hir.ok = false;
            self.hir.last = None;
            return;
        }
        let void = self.type_ctx.interner.void();
        self.hir.last = Some(HExpr::new(
            void,
            HExprKind::Print { arg: Box::new(arg), newline },
        ));
    }

    /// Records `recv.size()` (typed `int`): an array reads its stored length word (`ArrayLen`), while a
    /// string scans for its NUL terminator at runtime (`StrLen`), since the two have different layouts.
    pub(in crate::semantics::analyzer) fn hir_set_array_len(&mut self, recv: Option<HExpr>) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        match recv {
            Some(e) => {
                let int = self.type_ctx.interner.int();
                let is_string = matches!(
                    self.type_ctx.interner.kind(self.type_ctx.interner.strip_nullable(e.ty)),
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
    pub(in crate::semantics::analyzer) fn hir_set_is_type(&mut self, value: Option<HExpr>, target: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let bool_ty = self.type_ctx.interner.bool();
        let target_ty = self.type_ctx.lower(target);
        self.hir.last = value.map(|v| {
            HExpr::new(
                bool_ty,
                HExprKind::IsType { value: Box::new(v), target: target_ty },
            )
        });
    }

    /// Records string concatenation `a + b` (typed `string`): each non-string operand is first run
    /// through the object-protocol `to_string`, then the two string pointers are joined by the
    /// runtime `$concat_strings`. Drops out of coverage if either operand is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_concat(
        &mut self,
        lhs: Option<HExpr>,
        lhs_is_string: bool,
        rhs: Option<HExpr>,
        rhs_is_string: bool,
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
        let to_str = |e: HExpr, is_string: bool| {
            if is_string {
                e
            } else {
                HExpr::new(string, HExprKind::ToString(Box::new(e)))
            }
        };
        self.hir.last = Some(HExpr::new(
            string,
            HExprKind::Concat(
                Box::new(to_str(lhs, lhs_is_string)),
                Box::new(to_str(rhs, rhs_is_string)),
            ),
        ));
    }

    /// Records a C-style enum's `to_string()` (typed `string`): the backend maps the receiver's
    /// discriminant to its interned variant-name string via `arms` (`(discriminant, name)` for
    /// every member).
    pub(in crate::semantics::analyzer) fn hir_set_enum_name(&mut self, recv: Option<HExpr>, arms: Vec<(i64, String)>) {
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

    /// Records `Array.new<T>(len)` (typed `T[]`): a zero-initialized array allocation. Drops out of
    /// coverage if the length is not representable.
    /// Records an empty array literal `[]` of element type `elem_ty` as a zero-length allocation
    /// (equivalent to `Array.new<T>(0)`).
    pub(in crate::semantics::analyzer) fn hir_set_empty_array(&mut self, elem_ty: &Type) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let int = self.type_ctx.interner.int();
        let zero = HExpr::new(int, HExprKind::IntLit(0));
        self.hir_set_array_new(elem_ty, Some(zero));
    }

    pub(in crate::semantics::analyzer) fn hir_set_array_new(&mut self, elem_ty: &Type, len: Option<HExpr>) {
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

    /// Records `recv.char_at(idx)` (typed `char`): a runtime `$char_at` read. Drops out of coverage
    /// if either the receiver or the index is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_char_at(&mut self, recv: Option<HExpr>, idx: Option<HExpr>) {
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
    pub(in crate::semantics::analyzer) fn hir_set_await(&mut self, inner: Option<HExpr>, inner_ty: &Type) {
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
    /// Sets `last` to a read of an already-allocated local (used by the match-expression desugar to
    /// yield the result temporary as the match's value).
    pub(in crate::semantics::analyzer) fn hir_set_local_read(&mut self, local: LocalId, ty: TypeId) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        self.hir.last = Some(HExpr::new(ty, HExprKind::Var(Binding::Local(local))));
    }
}
