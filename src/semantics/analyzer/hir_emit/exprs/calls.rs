//! HIR for calls (direct/indirect/generic), function values, field/enum reads, constructors, and
//! method/interface/union construction.

use super::*;

impl<'a> Analyzer<'a> {
    /// Records the HIR for a direct free-function call `name(args)`. Resolves `name` to its function
    /// `DefId`; if it is not a registered (non-generic, non-overloaded) function or any argument is
    /// not representable, the call is dropped from coverage (the function falls back to the legacy
    /// path).
    pub(in crate::semantics::analyzer) fn hir_set_call(
        &mut self,
        name: &str,
        args: Vec<Option<HExpr>>,
        ret: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, name) else {
            self.hir.last = None;
            return;
        };
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
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
    pub(in crate::semantics::analyzer) fn hir_set_func_value(
        &mut self,
        name: &str,
        func_ty: &Type,
        ret: &Type,
    ) {
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
            HExprKind::Var(Binding::Func(Callee {
                def,
                instance: vec![],
                ret: ret_ty,
            })),
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
            HExprKind::Var(Binding::Func(Callee {
                def,
                instance,
                ret: ret_ty,
            })),
        ));
    }

    /// Records an indirect call `f(args)` where `f` is a function-typed local: the target reads the
    /// local (whose value is a function-table index) and the call dispatches through it. Drops coverage
    /// if the name is not a known local or any argument is not representable.
    pub(in crate::semantics::analyzer) fn hir_set_indirect_call(
        &mut self,
        name: &str,
        args: Vec<Option<HExpr>>,
        ret: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let Some(&(local, ty)) = self.hir.locals.get(name) else {
            self.hir.last = None;
            return;
        };
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
        let ret_ty = self.type_ctx.lower(ret);
        let target = HExpr::new(ty, HExprKind::Var(Binding::Local(local)));
        self.hir.last = Some(HExpr::new(
            ret_ty,
            HExprKind::IndirectCall {
                target: Box::new(target),
                args: collected,
            },
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
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
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
    pub(in crate::semantics::analyzer) fn hir_set_enum_value(
        &mut self,
        value: i64,
        enum_ty: &Type,
    ) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        let ty = self.type_ctx.lower(enum_ty);
        self.hir.last = Some(HExpr::new(ty, HExprKind::EnumValue(value)));
    }

    /// Records the HIR for a struct field read `obj.field`; `field` is the resolved field index
    /// (offset order). Fails over to the legacy path if the receiver was not representable.
    pub(in crate::semantics::analyzer) fn hir_set_field(
        &mut self,
        obj: Option<HExpr>,
        field: usize,
        field_ty: &Type,
    ) {
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
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
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
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
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
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
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
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
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
}
