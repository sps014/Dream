use super::*;

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
    pub(in crate::semantics::analyzer) fn hir_assign_local_id(&mut self, local: LocalId, value: Option<HExpr>) {
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
    pub(in crate::semantics::analyzer) fn hir_declare_local(&mut self, name: &str, ty: &Type, value: Option<HExpr>) {
        if !self.active() {
            return;
        }
        let Some(value) = value else {
            self.hir.ok = false;
            return;
        };
        let ty = self.type_ctx.lower(ty);
        let value = self.coerce_to(value, ty);
        let local = LocalId(self.hir.next_local);
        self.hir.next_local += 1;
        self.hir.locals.insert(name.to_string(), (local, ty));
        self.hir.local_decls.push(HLocal {
            id: local,
            name: name.to_string(),
            ty,
        });
        self.push_stmt(HStmt::Let { local, ty, value });
    }

    /// Inserts an implicit boxing cast when a primitive `value` is stored into an `object`-typed
    /// slot (`let o: object = 42`), so the backend boxes it rather than storing a raw scalar. All
    /// other conversions (reference→object, numeric widening) are left to the backend / call sites.
    fn coerce_to(&mut self, value: HExpr, target: TypeId) -> HExpr {
        use crate::types::{PrimTy, TyKind};
        // Snapshot the (nullable-stripped) kinds so the interner borrow does not outlive the branches
        // below that need `&mut self` (the `js` box/unbox helpers).
        let (target_k, val_k) = {
            let i = &self.type_ctx.interner;
            (
                i.kind(i.strip_nullable(target)).clone(),
                i.kind(i.strip_nullable(value.ty)).clone(),
            )
        };
        // Boxing a primitive into `object`.
        if matches!(target_k, TyKind::Object) && matches!(val_k, TyKind::Prim(_)) {
            return HExpr::new(target, HExprKind::Cast(Box::new(value)));
        }
        // Dynamic `js`: box a primitive/`string` into a `js` handle, or unbox a `js` value into a
        // primitive/`string`, at this typed binding boundary (so `let x: js = 5` and
        // `let n: int = el.count` work without an explicit conversion).
        if matches!(target_k, TyKind::Js) && matches!(val_k, TyKind::Prim(_)) {
            return self.box_to_js(value).unwrap_or_else(|| HExpr::new(target, HExprKind::IntLit(0)));
        }
        if matches!(val_k, TyKind::Js) && matches!(target_k, TyKind::Prim(_)) {
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
                let target_prim = self.type_ctx.interner.strip_nullable(target);
                return HExpr::new(target_prim, HExprKind::Cast(Box::new(value)));
            }
        }
        value
    }

    /// Appends an assignment to a local or module-global. Fails the function for an unresolved name
    /// or a non-representable value.
    pub(in crate::semantics::analyzer) fn hir_assign_local(&mut self, name: &str, value: Option<HExpr>) {
        if !self.active() {
            return;
        }
        let Some(value) = value else {
            self.hir.ok = false;
            return;
        };
        if let Some(&(local, _)) = self.hir.locals.get(name) {
            self.push_stmt(HStmt::Assign {
                place: HPlace::Local(local),
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

    /// Appends `return value;`, failing the function if the value was not representable.
    pub(in crate::semantics::analyzer) fn hir_return_value(&mut self, value: Option<HExpr>) {
        if !self.active() {
            return;
        }
        match value {
            Some(value) => self.push_stmt(HStmt::Return(Some(value))),
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
    pub(in crate::semantics::analyzer) fn hir_while(&mut self, cond: Option<HExpr>, body: Vec<HStmt>, label: Option<String>) {
        if !self.active() {
            return;
        }
        match cond {
            Some(cond) => self.push_stmt(HStmt::While { cond, body, label }),
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

    /// Appends a desugared `for (init; cond; step) { body }`. `init`/`step` must each be exactly one
    /// statement (the surface form guarantees this) and `cond` must be present.
    pub(in crate::semantics::analyzer) fn hir_for(
        &mut self,
        mut init: Vec<HStmt>,
        cond: Option<HExpr>,
        mut step: Vec<HStmt>,
        body: Vec<HStmt>,
        label: Option<String>,
    ) {
        if !self.active() {
            return;
        }
        match (init.len(), step.len(), cond) {
            (1, 1, Some(cond)) => self.push_stmt(HStmt::For {
                init: Box::new(init.remove(0)),
                cond,
                step: Box::new(step.remove(0)),
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
            (Some(elem), Some(iterable)) => {
                self.push_stmt(HStmt::Foreach {
                    elem,
                    iterable,
                    body,
                    label,
                })
            }
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
    pub(in crate::semantics::analyzer) fn hir_const_arm(&self, label: Option<HExpr>, body: Vec<HStmt>) -> Option<HArm> {
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
