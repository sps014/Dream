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

    /// The `__Cell<elem>` struct type wrapping a captured local of type `elem` (see
    /// `src/stdlib/core/closure.dream`) — the shared mutable box every reader of a captured name
    /// (the enclosing function and every closure that captures it) reads/writes through.
    pub(in crate::semantics::analyzer) fn cell_type(elem: &Type) -> Type {
        Self::boxed_type("__Cell", elem)
    }

    /// The `__RefBox<elem>` value-struct type wrapping a local/parameter that is `ref`-passed but
    /// never closure-captured (see `src/stdlib/core/closure.dream` and `docs/compiler/03-hir.md`).
    pub(in crate::semantics::analyzer) fn ref_box_type(elem: &Type) -> Type {
        Self::boxed_type("__RefBox", elem)
    }

    fn boxed_type(base: &str, elem: &Type) -> Type {
        Type::Struct(
            crate::semantics::analyzer::synthetic_token(TokenKind::IdentifierToken, base),
            Some(vec![elem.clone()]),
        )
    }

    /// Builds `__Cell<elem_ty>(value)`: ensures that instantiation of the generic `__Cell<T>` class
    /// exists (registering it on first use, exactly like an ordinary `List<T>()` construction site),
    /// then emits the `New` HIR wrapping `value`. Used both for a captured `let`/parameter (see
    /// `hir_declare_local`) and — indirectly, by construction at the capturing lambda's use site —
    /// for the environment word handed to `build_funcbox`.
    pub(in crate::semantics::analyzer) fn hir_build_cell_new(
        &mut self,
        elem_ty: &Type,
        value: HExpr,
    ) -> Option<HExpr> {
        self.hir_build_boxed_new("__Cell", Self::cell_type(elem_ty), elem_ty, value)
    }

    /// Builds `__RefBox<elem_ty>(value)`, the same shape as [`Self::hir_build_cell_new`] but backed
    /// by the value-struct box a purely `ref`-passed (not closure-captured) name uses instead of the
    /// heap `__Cell<T>` — see `hir_declare_local`/`Analyzer::hir_begin_function`.
    pub(in crate::semantics::analyzer) fn hir_build_ref_box_new(
        &mut self,
        elem_ty: &Type,
        value: HExpr,
    ) -> Option<HExpr> {
        self.hir_build_boxed_new("__RefBox", Self::ref_box_type(elem_ty), elem_ty, value)
    }

    fn hir_build_boxed_new(
        &mut self,
        base: &str,
        boxed_ty: Type,
        elem_ty: &Type,
        value: HExpr,
    ) -> Option<HExpr> {
        use crate::syntax::nodes::types::mangle_generic;
        use crate::types::constructor_fn;
        let mut throwaway = crate::diagnostics::DiagnosticBag::new(None);
        let no_span = TextSpan {
            start: 0,
            end: 0,
            line_no: 0,
            col_no: 0,
        };
        self.ensure_struct_instantiated(
            base,
            std::slice::from_ref(elem_ty),
            &no_span,
            &mut throwaway,
        );
        let mangled = mangle_generic(base, std::slice::from_ref(elem_ty));
        let def = self.type_ctx.defs.lookup(DefKind::Struct, base)?;
        let ctor = self
            .type_ctx
            .defs
            .lookup(DefKind::Function, &constructor_fn(&mangled));
        let ty = self.type_ctx.lower(&boxed_ty);
        Some(HExpr::new(
            ty,
            HExprKind::New {
                def,
                instance: vec![],
                ctor,
                args: vec![value],
            },
        ))
    }

    /// Reads the `$__closure_env` module global (an `int`): the environment word a caller sets
    /// (see `hir_set_indirect_call_expr`) just before an indirect call through a boxed closure
    /// value, which the callee's own prologue reads here to recover its captured cells (see
    /// `Analyzer::hir_begin_function`'s capturing-lambda prologue).
    pub(in crate::semantics::analyzer) fn hir_read_closure_env(&mut self) -> Option<HExpr> {
        let &(env_global, ty) = self.hir.globals.get("__closure_env")?;
        Some(HExpr::new(ty, HExprKind::Var(Binding::Global(env_global))))
    }

    /// Looks up one of the `__Closure.*` compiler-internal intrinsics (see
    /// `src/stdlib/core/closure.dream`) by its bare method name. These back the `fun(...)` closure
    /// ABI (a boxed `[funcidx][env]` heap value); every call site is built directly here rather than
    /// through ordinary name resolution, so the stdlib class is never referenced by user code.
    fn closure_intrinsic(&self, method: &str) -> Option<DefId> {
        self.type_ctx.defs.lookup(
            DefKind::Function,
            &crate::types::method_fn("__Closure", method),
        )
    }

    /// Wraps a raw function-table index (`raw`, always `int`-typed) plus an optional environment
    /// pointer (`env`, `None` for a non-capturing value — becomes a `0` literal) into a boxed
    /// `fun(...)` value via `__Closure.funcbox_new`, typed as `func_ty` (the box is a plain `i32` at
    /// runtime regardless of the declared `fun(...)` shape it carries). Returns `None` (dropping HIR
    /// coverage) if the `__Closure` intrinsics are not registered — should not happen, since the
    /// stdlib prelude always defines them, but this keeps the failure a silent coverage drop rather
    /// than a panic if the prelude is ever missing them.
    fn build_funcbox(&mut self, raw: HExpr, env: Option<HExpr>, func_ty: &Type) -> Option<HExpr> {
        let new_def = self.closure_intrinsic("funcbox_new")?;
        let int_ty = self.type_ctx.interner.int();
        let env_expr = env.unwrap_or_else(|| HExpr::new(int_ty, HExprKind::IntLit(0)));
        let box_ty = self.type_ctx.lower(func_ty);
        let callee = Callee {
            def: new_def,
            instance: vec![],
            ret: box_ty,
        };
        Some(HExpr::new(
            box_ty,
            HExprKind::Call {
                callee,
                args: vec![raw, env_expr],
            },
        ))
    }

    /// Extracts a boxed `fun(...)` value's raw function-table index, discarding its environment word
    /// — used at boundaries with no env-restoring prologue of their own (a host `@js` bridge; see
    /// `js_interop::box_to_js`), where only the funcidx half of the box is meaningful. `None` if the
    /// `__Closure` intrinsics are unavailable.
    pub(in crate::semantics::analyzer) fn hir_funcbox_funcidx(
        &mut self,
        boxed: HExpr,
    ) -> Option<HExpr> {
        let def = self.closure_intrinsic("funcbox_funcidx")?;
        let int_ty = self.type_ctx.interner.int();
        Some(HExpr::new(
            int_ty,
            HExprKind::Call {
                callee: Callee {
                    def,
                    instance: vec![],
                    ret: int_ty,
                },
                args: vec![boxed],
            },
        ))
    }

    /// Records a first-class function value: a bare function name used as a value (e.g. `let f = foo;`
    /// or passing `foo` to a `fun(...)` parameter) resolves its `Binding::Func` (the def + signature)
    /// to a raw function-table index, then boxes it (with a null environment — see [`build_funcbox`])
    /// so it carries the same runtime shape as a capturing closure. Drops coverage if the name is not
    /// a registered function def.
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
        let int_ty = self.type_ctx.interner.int();
        let ret_ty = self.type_ctx.lower(ret);
        let raw = HExpr::new(
            int_ty,
            HExprKind::Var(Binding::Func(Callee {
                def,
                instance: vec![],
                ret: ret_ty,
            })),
        );
        self.hir.last = self.build_funcbox(raw, None, func_ty);
    }

    /// Appends `__Closure.retain(env);` as a statement: bumps the captured environment's (a
    /// `__Cell<T>`, reinterpreted as `object`) reference count with no matching release, so it
    /// outlives the enclosing function's own scope-exit release of its `let`/parameter slot — see
    /// `src/stdlib/core/closure.dream`'s doc comment for the full leak-vs-dangle tradeoff this
    /// buys. A no-op (returns `false`) if the `__Closure` intrinsics are unavailable.
    pub(in crate::semantics::analyzer) fn hir_retain_env(&mut self, env: HExpr) -> bool {
        let Some(def) = self.closure_intrinsic("retain") else {
            return false;
        };
        let void_ty = self.type_ctx.interner.void();
        let object_ty = self.type_ctx.interner.object();
        let arg = HExpr::new(object_ty, HExprKind::Cast(Box::new(env)));
        self.push_stmt(HStmt::Expr(HExpr::new(
            void_ty,
            HExprKind::Call {
                callee: Callee {
                    def,
                    instance: vec![],
                    ret: void_ty,
                },
                args: vec![arg],
            },
        )));
        true
    }

    /// Like [`hir_set_func_value`], but wraps the box around a *captured* environment (a
    /// `__Cell<T>` pointer, reinterpreted to `int` — see [`build_funcbox`]) instead of a null one:
    /// the lambda's own lifted function reads it back apart at its own prologue (see
    /// `Analyzer::hir_begin_function`). Drops coverage if the name is not a registered function def.
    pub(in crate::semantics::analyzer) fn hir_set_capturing_func_value(
        &mut self,
        name: &str,
        env_cell: HExpr,
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
        let int_ty = self.type_ctx.interner.int();
        let ret_ty = self.type_ctx.lower(ret);
        let raw = HExpr::new(
            int_ty,
            HExprKind::Var(Binding::Func(Callee {
                def,
                instance: vec![],
                ret: ret_ty,
            })),
        );
        let env_int = HExpr::new(int_ty, HExprKind::Cast(Box::new(env_cell)));
        self.hir.last = self.build_funcbox(raw, Some(env_int), func_ty);
    }

    /// Like [`hir_set_capturing_func_value`], but for **two or more** captured names: the
    /// environment is an `object[]` array (one slot per capture, in `env_cells`' order) rather than
    /// a single `__Cell<T>` — see the lifted function's receiving half,
    /// `Analyzer::receive_closure_captures`. Each cell is written into the array as an ordinary
    /// `object[]` store, so the emitter's normal container-store rule retains it on the array's
    /// behalf (see `mir::passes::rc`'s doc comment) — only the array itself, not each individual
    /// cell, needs the explicit permanent [`Self::hir_retain_env`] to outlive this scope's exit.
    pub(in crate::semantics::analyzer) fn hir_set_multi_capturing_func_value(
        &mut self,
        name: &str,
        env_cells: Vec<HExpr>,
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
        let int_ty = self.type_ctx.interner.int();
        let object_ty = self.type_ctx.interner.object();
        let array_ty = self.type_ctx.interner.array(object_ty);
        let len = HExpr::new(int_ty, HExprKind::IntLit(env_cells.len() as i64));
        let array_new = HExpr::new(
            array_ty,
            HExprKind::ArrayNew {
                elem_ty: object_ty,
                len: Box::new(len),
            },
        );
        let array_local = LocalId(self.hir.next_local);
        self.hir.next_local += 1;
        self.hir.local_decls.push(HLocal {
            id: array_local,
            name: "__closure_env_array".to_string(),
            ty: array_ty,
        });
        self.push_stmt(HStmt::Let {
            local: array_local,
            ty: array_ty,
            value: array_new,
        });
        let array_read = || HExpr::new(array_ty, HExprKind::Var(Binding::Local(array_local)));
        for (i, cell) in env_cells.into_iter().enumerate() {
            let index = HExpr::new(int_ty, HExprKind::IntLit(i as i64));
            let value = HExpr::new(object_ty, HExprKind::Cast(Box::new(cell)));
            self.push_stmt(HStmt::Assign {
                place: HPlace::Index {
                    array: Box::new(array_read()),
                    index: Box::new(index),
                },
                value,
            });
        }
        self.hir_retain_env(array_read());

        let ret_ty = self.type_ctx.lower(ret);
        let raw = HExpr::new(
            int_ty,
            HExprKind::Var(Binding::Func(Callee {
                def,
                instance: vec![],
                ret: ret_ty,
            })),
        );
        let env_int = HExpr::new(int_ty, HExprKind::Cast(Box::new(array_read())));
        self.hir.last = self.build_funcbox(raw, Some(env_int), func_ty);
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
        let int_ty = self.type_ctx.interner.int();
        let ret_ty = self.type_ctx.lower(ret);
        let raw = HExpr::new(
            int_ty,
            HExprKind::Var(Binding::Func(Callee {
                def,
                instance,
                ret: ret_ty,
            })),
        );
        self.hir.last = self.build_funcbox(raw, None, func_ty);
    }

    /// Records an indirect call `f(args)` where `f` is a function-typed local: unboxes `f` (see
    /// `build_funcbox`) — publishing its environment word to `$__closure_env` (read by a capturing
    /// callee's own prologue; see `hir_read_closure_env`) and extracting its function-table index —
    /// then dispatches through that index. `f` is read into a fresh local first so both extractions
    /// (env, funcidx) observe the same box value without re-evaluating `f` twice. Drops coverage if
    /// the name is not a known local, the `__Closure` intrinsics are unavailable, or any argument is
    /// not representable.
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
        let target = HExpr::new(ty, HExprKind::Var(Binding::Local(local)));
        self.hir_set_indirect_call_expr(target, args, ret);
    }

    /// Shared unboxing logic for an indirect call through a boxed `fun(...)` value `boxed` — see
    /// [`hir_set_indirect_call`]. Split out so a closure-valued *expression* (not just a named local)
    /// can dispatch through the same path once general `fun(...)`-typed expressions (not just
    /// locals) support calling — currently only [`hir_set_indirect_call`] calls this.
    fn hir_set_indirect_call_expr(&mut self, boxed: HExpr, args: Vec<Option<HExpr>>, ret: &Type) {
        let (Some(funcidx_def), Some(env_def)) = (
            self.closure_intrinsic("funcbox_funcidx"),
            self.closure_intrinsic("funcbox_env"),
        ) else {
            self.hir.last = None;
            return;
        };
        let Some(collected) = Self::collect_hir_args(args) else {
            self.hir.last = None;
            return;
        };
        let int_ty = self.type_ctx.interner.int();
        let box_ty = boxed.ty;
        // Materialize the box into a fresh local so both reads below see the same value.
        let box_local = LocalId(self.hir.next_local);
        self.hir.next_local += 1;
        self.hir.local_decls.push(HLocal {
            id: box_local,
            name: "__closure_box".to_string(),
            ty: box_ty,
        });
        self.push_stmt(HStmt::Let {
            local: box_local,
            ty: box_ty,
            value: boxed,
        });
        let box_read = || HExpr::new(box_ty, HExprKind::Var(Binding::Local(box_local)));

        let Some(&(env_global, _)) = self.hir.globals.get("__closure_env") else {
            self.hir.last = None;
            return;
        };
        let env_call = HExpr::new(
            int_ty,
            HExprKind::Call {
                callee: Callee {
                    def: env_def,
                    instance: vec![],
                    ret: int_ty,
                },
                args: vec![box_read()],
            },
        );
        self.push_stmt(HStmt::Assign {
            place: HPlace::Global(env_global),
            value: env_call,
        });

        // `target`'s type must stay `box_ty` (the `fun(...)` shape, e.g. `fun(int,int):int`), *not*
        // `int` — despite the funcidx being a plain `i32` at runtime, the emitter derives the
        // `call_indirect` signature from this type (see `func_sig`/`Rvalue::IndirectCall` emission),
        // so mislabeling it as `int` would emit the wrong (fallback) `call_indirect` type immediate.
        let funcidx_call = HExpr::new(
            box_ty,
            HExprKind::Call {
                callee: Callee {
                    def: funcidx_def,
                    instance: vec![],
                    ret: int_ty,
                },
                args: vec![box_read()],
            },
        );
        let ret_ty = self.type_ctx.lower(ret);
        self.hir.last = Some(HExpr::new(
            ret_ty,
            HExprKind::IndirectCall {
                target: Box::new(funcidx_call),
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
