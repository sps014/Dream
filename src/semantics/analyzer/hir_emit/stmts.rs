use super::*;

/// Which box a boxed local's slot holds — see `Analyzer::hir_declare_local`.
enum BoxKind {
    /// `__Cell<T>`: heap-allocated, ARC-managed. Used when the name is captured by a lambda (its
    /// storage may need to outlive this function's stack frame).
    Cell,
    /// `__RefBox<T>`: a stack-resident value struct. Used when the name is only ever `ref`-passed
    /// (never captured) — its storage never needs to outlive this call, so no heap allocation or
    /// ARC bookkeeping is needed.
    RefBox,
}

impl<'a> Analyzer<'a> {
    /// Appends `await e;` at statement position.
    pub(in crate::semantics::analyzer) fn hir_await_stmt(&mut self, value: Option<HExpr>) {
        if !self.active() {
            return;
        }
        match value {
            Some(v) => self.push_stmt(HStmt::Await(v)),
            None => self.hir.ok = false,
        }
    }
    /// Appends an assignment to an already-allocated local slot (used by the match-expression
    /// desugar's result temporary).
    pub(in crate::semantics::analyzer) fn hir_assign_local_id(
        &mut self,
        local: LocalId,
        value: Option<HExpr>,
    ) {
        if !self.active() {
            return;
        }
        match value {
            Some(value) => self.push_stmt(HStmt::Assign {
                place: HPlace::Local(local),
                value,
            }),
            None => self.hir.ok = false,
        }
    }

    /// Appends a field assignment `obj.field = value;` (`field` is the resolved offset-order index).
    pub(in crate::semantics::analyzer) fn hir_assign_field(
        &mut self,
        obj: Option<HExpr>,
        field: usize,
        value: Option<HExpr>,
    ) {
        if !self.active() {
            return;
        }
        match (obj, value) {
            (Some(obj), Some(value)) => self.push_stmt(HStmt::Assign {
                place: HPlace::Field {
                    obj: Box::new(obj),
                    field,
                },
                value,
            }),
            _ => self.hir.ok = false,
        }
    }

    /// Appends an indexed assignment `array[index] = value;`.
    pub(in crate::semantics::analyzer) fn hir_assign_index(
        &mut self,
        array: Option<HExpr>,
        index: Option<HExpr>,
        value: Option<HExpr>,
    ) {
        if !self.active() {
            return;
        }
        match (array, index, value) {
            (Some(array), Some(index), Some(value)) => self.push_stmt(HStmt::Assign {
                place: HPlace::Index {
                    array: Box::new(array),
                    index: Box::new(index),
                },
                value,
            }),
            _ => self.hir.ok = false,
        }
    }

    /// Appends a `let` binding, allocating a fresh local slot. Fails the function if the initializer
    /// was not representable.
    ///
    /// If `name` is captured by a lambda elsewhere in this function's body (`self.boxed_locals`,
    /// computed by the capture-scan pre-pass in `hir_begin_function`), the local is boxed instead:
    /// the slot holds a `__Cell<T>` object (constructed here around `value`) rather than a raw `T`,
    /// so a later capturing closure can alias the very same storage (see
    /// `expressions::lambda`/`hir_read_capture_cell`). If `name` is only ever `ref`-passed (never
    /// captured, `self.ref_boxed_locals`), it is boxed into `__RefBox<T>` instead — a stack-resident
    /// value struct, since its storage never needs to outlive this call (see `BoxKind`). Either way
    /// `self.hir.boxed` records the unboxed element type so subsequent reads/writes of `name`
    /// (`hir_set_var`/`hir_assign_local`) know to go through the box's `.value` field transparently
    /// — every other analyzer-facing type stays `T`.
    pub(in crate::semantics::analyzer) fn hir_declare_local(
        &mut self,
        name: &str,
        ty: &Type,
        value: Option<HExpr>,
    ) {
        if !self.active() {
            return;
        }
        let Some(value) = value else {
            self.hir.ok = false;
            return;
        };
        if self.boxed_locals.contains(name) {
            self.declare_boxed_local(name, ty, value, BoxKind::Cell);
            return;
        }
        if self.ref_boxed_locals.contains(name) {
            self.declare_boxed_local(name, ty, value, BoxKind::RefBox);
            return;
        }
        let ty_id = self.type_ctx.lower(ty);
        let value = self.coerce_to(value, ty_id);
        let local = LocalId(self.hir.next_local);
        self.hir.next_local += 1;
        self.hir.locals.insert(name.to_string(), (local, ty_id));
        self.hir.local_decls.push(HLocal {
            id: local,
            name: name.to_string(),
            ty: ty_id,
        });
        self.push_stmt(HStmt::Let {
            local,
            ty: ty_id,
            value,
        });
    }

    fn declare_boxed_local(&mut self, name: &str, ty: &Type, value: HExpr, kind: BoxKind) {
        let (cell, cell_ty) = match kind {
            BoxKind::Cell => (self.hir_build_cell_new(ty, value), Self::cell_type(ty)),
            BoxKind::RefBox => (
                self.hir_build_ref_box_new(ty, value),
                Self::ref_box_type(ty),
            ),
        };
        let Some(cell) = cell else {
            self.hir.ok = false;
            return;
        };
        let cell_tid = self.type_ctx.lower(&cell_ty);
        let elem_tid = self.type_ctx.lower(ty);
        let local = LocalId(self.hir.next_local);
        self.hir.next_local += 1;
        self.hir.locals.insert(name.to_string(), (local, cell_tid));
        self.hir.local_decls.push(HLocal {
            id: local,
            name: name.to_string(),
            ty: cell_tid,
        });
        self.hir.boxed.insert(name.to_string(), elem_tid);
        self.push_stmt(HStmt::Let {
            local,
            ty: cell_tid,
            value: cell,
        });
    }

    /// Inserts an implicit boxing cast when a primitive `value` is stored into an `object`-typed
    /// slot (`let o: object = 42`), so the backend boxes it rather than storing a raw scalar. All
    /// other conversions (reference→object, numeric widening) are left to the backend / call sites.
    fn coerce_to(&mut self, value: HExpr, target: TypeId) -> HExpr {
        use crate::types::{PrimTy, TyKind};
        // Snapshot the kinds so the interner borrow does not outlive the branches below that need
        // `&mut self` (the `js` box/unbox helpers).
        let (target_k, val_k) = {
            let i = &self.type_ctx.interner;
            (i.kind(target).clone(), i.kind(value.ty).clone())
        };
        // Boxing a primitive into `object`.
        if matches!(target_k, TyKind::Object) && matches!(val_k, TyKind::Prim(_)) {
            return HExpr::new(target, HExprKind::Cast(Box::new(value)));
        }
        // Boxing a value struct into `object` or an interface reference: the backend allocates a
        // tagged heap copy so the value participates in dynamic dispatch and the object protocol.
        // Reference structs are already pointers (identity upcast), so only value structs box here.
        if matches!(target_k, TyKind::Object | TyKind::Interface(..))
            && matches!(val_k, TyKind::Struct(..))
            && self.type_ctx.interner.is_value_type(value.ty)
        {
            return HExpr::new(target, HExprKind::Cast(Box::new(value)));
        }
        // Dynamic `js`: box a primitive/`string` into a `js` handle, or unbox a `js` value into a
        // primitive/`string`, at this typed binding boundary (so `let x: js = 5` and
        // `let n: int = el.count` work without an explicit conversion).
        if matches!(target_k, TyKind::Js) && matches!(val_k, TyKind::Prim(_) | TyKind::Struct(..)) {
            return self
                .box_to_js(value, None, None)
                .unwrap_or_else(|| HExpr::new(target, HExprKind::IntLit(0)));
        }
        if matches!(val_k, TyKind::Js) && matches!(target_k, TyKind::Prim(_) | TyKind::Struct(..)) {
            return self.unbox_from_js(value, target);
        }
        // Implicit numeric widening (e.g. `let w: long = 5;`, `let d: double = someLong;`). The two
        // primitives have different WASM representations (i32/i64/f64), so an explicit conversion must
        // be materialized; `emit_cast`/`numeric_conv` picks the right (un)signed extend/convert.
        let is_num = |p: PrimTy| {
            matches!(
                p,
                PrimTy::Int
                    | PrimTy::UInt
                    | PrimTy::Long
                    | PrimTy::ULong
                    | PrimTy::Byte
                    | PrimTy::Float
                    | PrimTy::Double
            )
        };
        if let (TyKind::Prim(tp), TyKind::Prim(vp)) = (target_k, val_k) {
            if tp != vp && is_num(tp) && is_num(vp) {
                let target_prim = target;
                return HExpr::new(target_prim, HExprKind::Cast(Box::new(value)));
            }
        }
        value
    }

    /// Appends an assignment to a local or module-global. Fails the function for an unresolved name
    /// or a non-representable value.
    ///
    /// A captured local (`self.hir.boxed`, see `hir_declare_local`) writes through its `__Cell<T>`
    /// box's `.value` field instead of the plain slot, so the write is visible to every closure
    /// (and the enclosing function itself) sharing that same cell.
    pub(in crate::semantics::analyzer) fn hir_assign_local(
        &mut self,
        name: &str,
        value: Option<HExpr>,
    ) {
        if !self.active() {
            return;
        }
        let Some(value) = value else {
            self.hir.ok = false;
            return;
        };
        if let Some(&cell_local) = self.hir.locals.get(name).map(|(l, _)| l) {
            if self.hir.boxed.contains_key(name) {
                let cell_tid = self.hir.locals.get(name).map(|(_, t)| *t).unwrap();
                let obj = HExpr::new(cell_tid, HExprKind::Var(Binding::Local(cell_local)));
                self.push_stmt(HStmt::Assign {
                    place: HPlace::Field {
                        obj: Box::new(obj),
                        field: 0,
                    },
                    value,
                });
                return;
            }
            self.push_stmt(HStmt::Assign {
                place: HPlace::Local(cell_local),
                value,
            });
        } else if let Some(&(global, _)) = self.hir.globals.get(name) {
            self.push_stmt(HStmt::Assign {
                place: HPlace::Global(global),
                value,
            });
        } else {
            self.hir.ok = false;
        }
    }

    /// Appends `return value;`, failing the function if the value was not representable. When
    /// `target` is given (the enclosing function's declared return type), `value` is first run
    /// through the same implicit-conversion path as a typed `let`/assignment (`coerce_to`) — most
    /// importantly numeric widening (`return 2.5;` in a `double`-returning function), which a bare
    /// literal's own type would otherwise leave narrower than the function's signature and desync
    /// from every backend site that trusts the declared return type (including, for `async fun`,
    /// the scheduler's single-width `Future.result` slot).
    pub(in crate::semantics::analyzer) fn hir_return_value(
        &mut self,
        value: Option<HExpr>,
        target: Option<TypeId>,
    ) {
        if !self.active() {
            return;
        }
        match value {
            Some(value) => {
                let value = match target {
                    Some(t) => self.coerce_to(value, t),
                    None => value,
                };
                self.push_stmt(HStmt::Return(Some(value)));
            }
            None => self.hir.ok = false,
        }
    }

    /// Appends a bare `return;`.
    pub(in crate::semantics::analyzer) fn hir_return_void(&mut self) {
        self.push_stmt(HStmt::Return(None));
    }

    /// Appends an expression statement, failing the function if it was not representable.
    pub(in crate::semantics::analyzer) fn hir_expr_stmt(&mut self, value: Option<HExpr>) {
        if !self.active() {
            return;
        }
        match value {
            Some(value) => self.push_stmt(HStmt::Expr(value)),
            None => self.hir.ok = false,
        }
    }

    /// Appends a `while (cond) { body }`. Fails the function if the condition was not representable.
    pub(in crate::semantics::analyzer) fn hir_while(
        &mut self,
        cond: Option<HExpr>,
        body: Vec<HStmt>,
        label: Option<String>,
    ) {
        if !self.active() {
            return;
        }
        match cond {
            Some(cond) => self.push_stmt(HStmt::While { cond, body, label }),
            None => self.hir.ok = false,
        }
    }

    /// Appends a `lock (target) { body }`. Fails the function if `target` was not representable.
    pub(in crate::semantics::analyzer) fn hir_lock(&mut self, target: Option<HExpr>, body: Vec<HStmt>) {
        if !self.active() {
            return;
        }
        match target {
            Some(target) => self.push_stmt(HStmt::Lock { target, body }),
            None => self.hir.ok = false,
        }
    }

    /// Appends a `do { body } while (cond)`. Fails the function if the condition was not
    /// representable.
    pub(in crate::semantics::analyzer) fn hir_do_while(
        &mut self,
        cond: Option<HExpr>,
        body: Vec<HStmt>,
        label: Option<String>,
    ) {
        if !self.active() {
            return;
        }
        match cond {
            Some(cond) => self.push_stmt(HStmt::DoWhile { cond, body, label }),
            None => self.hir.ok = false,
        }
    }

    /// Appends a desugared `for (init; cond; step) { body }`. `init`/`step` are statement lists
    /// (surface form contributes one real statement each, plus optional [`HStmt::SourceLine`] /
    /// [`HStmt::DebugLine`] markers from `hir_mark_line`). `cond` must be present.
    pub(in crate::semantics::analyzer) fn hir_for(
        &mut self,
        init: Vec<HStmt>,
        cond: Option<HExpr>,
        step: Vec<HStmt>,
        body: Vec<HStmt>,
        label: Option<String>,
    ) {
        if !self.active() {
            return;
        }
        let real = |stmts: &[HStmt]| {
            stmts
                .iter()
                .filter(|s| !matches!(s, HStmt::SourceLine(_) | HStmt::DebugLine(_)))
                .count()
        };
        match (real(&init), real(&step), cond) {
            (1, 1, Some(cond)) => self.push_stmt(HStmt::For {
                init,
                cond,
                step,
                body,
                label,
            }),
            _ => self.hir.ok = false,
        }
    }

    /// Appends `foreach (elem in iterable) { body }`. `elem` is the slot allocated (before the body
    /// was analyzed, so the body can resolve the element) via [`Self::hir_alloc_local`].
    pub(in crate::semantics::analyzer) fn hir_foreach(
        &mut self,
        elem: Option<LocalId>,
        iterable: Option<HExpr>,
        body: Vec<HStmt>,
        label: Option<String>,
    ) {
        if !self.active() {
            return;
        }
        match (elem, iterable) {
            (Some(elem), Some(iterable)) => self.push_stmt(HStmt::Foreach {
                elem,
                iterable,
                body,
                label,
            }),
            _ => self.hir.ok = false,
        }
    }

    /// Appends a `break`/`continue` (with optional loop label).
    pub(in crate::semantics::analyzer) fn hir_break(&mut self, label: Option<String>) {
        self.push_stmt(HStmt::Break(label));
    }

    pub(in crate::semantics::analyzer) fn hir_continue(&mut self, label: Option<String>) {
        self.push_stmt(HStmt::Continue(label));
    }

    /// Appends a `switch`/statement-`match` lowered to [`HStmt::Switch`]. `arms` are the already-built
    /// pattern/body pairs and `default` the fallthrough block. `ok` is the caller's verdict on
    /// whether every arm was representable (e.g. no multi-label case, scrutinee present); a `false`
    /// verdict, a missing scrutinee, or inactive collection fails the function.
    pub(in crate::semantics::analyzer) fn hir_switch(
        &mut self,
        scrutinee: Option<HExpr>,
        arms: Vec<HArm>,
        default: Vec<HStmt>,
        ok: bool,
    ) {
        if !self.active() {
            return;
        }
        match scrutinee {
            Some(scrutinee) if ok => self.push_stmt(HStmt::Switch {
                scrutinee,
                arms,
                default,
            }),
            _ => self.hir.ok = false,
        }
    }

    /// Builds a `Const` switch arm from a label expression (the case value).
    pub(in crate::semantics::analyzer) fn hir_const_arm(
        &self,
        label: Option<HExpr>,
        body: Vec<HStmt>,
    ) -> Option<HArm> {
        label.map(|label| HArm {
            pattern: HPattern::Const(label),
            body,
        })
    }

    /// Builds a `Variant` match arm (`Enum.Variant(bindings...) => body`). `bindings` are the local
    /// slots already allocated for the payload (in field order).
    pub(in crate::semantics::analyzer) fn hir_variant_arm(
        &self,
        def: DefId,
        variant: usize,
        bindings: Vec<LocalId>,
        body: Vec<HStmt>,
    ) -> HArm {
        HArm {
            pattern: HPattern::Variant {
                def,
                variant,
                bindings,
            },
            body,
        }
    }
}
