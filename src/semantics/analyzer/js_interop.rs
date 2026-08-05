//! Desugaring of native syntax on the dynamic `js` type into calls to the stdlib interop bridges
//! declared in `src/stdlib/core/js.dream`.
//!
//! When a receiver has type `js`, member access, method calls, indexing, property assignment, and
//! calling the value itself all bind *dynamically*: the compiler performs no member resolution and
//! instead lowers the operation to a fixed `__*` bridge extern, marshaling arguments through a
//! single host call. Variadic call/invoke arguments are collected into a `js[]` (each argument
//! boxed to a `js` handle); the host reads it and applies the JS call in one boundary crossing.
//!
//! Every dynamic operation yields `js`; conversions back to Dream values happen at typed boundaries
//! (see the box/unbox helpers, also used by `coerce_to`) or via the explicit `js.to_int()` etc.

use super::synthetic_token;
use crate::diagnostics::DiagnosticBag;
use crate::hir::{Binding, Callee, HExpr, HExprKind};
use crate::semantics::analyzer::Analyzer;
use crate::semantics::errors::SemanticError;
use crate::syntax::nodes::{ExpressionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::text::text_span::TextSpan;
use crate::types::{method_fn, DefId, DefKind, PrimTy, TyKind, TypeId};

impl<'a> Analyzer<'a> {
    /// The legacy AST `Type` for the dynamic `js` type (a bare nominal name the type context lowers
    /// to `TyKind::Js`).
    pub(super) fn js_type() -> Type {
        Type::Struct(
            synthetic_token(TokenKind::IdentifierToken, crate::mir::js_abi::JS_TYPE),
            None,
        )
    }

    /// True if `ty` is the dynamic `js` type. `js` is represented at the AST layer as a nominal type
    /// whose spelling is exactly [`js_abi::JS_TYPE`](crate::mir::js_abi::JS_TYPE); comparing against
    /// that shared constant (rather than a bare literal) keeps recognition in lockstep with the
    /// bridge-mangling side, and the exact match excludes `js[]` / `js?`.
    pub(super) fn is_js_type(&self, ty: &Type) -> bool {
        ty.get_type() == crate::mir::js_abi::JS_TYPE
    }

    /// Diagnostic when a capturing `fun(...)` value is handed to a JS API. The host bridges
    /// (`func0`/`func`/`__funcN`, FUNC slots) only take the funcidx half of a funcbox — the env
    /// word is discarded — so a capturing lambda would lose its environment.
    const JS_CAPTURING_CALLBACK_MSG: &'static str = "capturing lambdas cannot be passed to JS APIs (the closure environment would be lost); pass a non-capturing top-level function, or wrap only a captureless `fun(...)` via `js.func` / `js.func0`";

    /// True when `e` is a known-capturing `fun(...)` value: a `funcbox_new` with a non-zero env,
    /// a `Binding::Func` whose def is a capturing lambda/method-group, or a fun-typed local marked
    /// capturing in [`Self::capturing_fun_locals`].
    pub(in crate::semantics::analyzer) fn func_expr_is_capturing(&self, e: &HExpr) -> bool {
        match &e.kind {
            HExprKind::Cast(inner) => self.func_expr_is_capturing(inner),
            HExprKind::Call { callee, args } => {
                if self.closure_intrinsic("funcbox_new") == Some(callee.def) && args.len() >= 2 {
                    let env_nonzero = !matches!(args[1].kind, HExprKind::IntLit(0));
                    if env_nonzero {
                        return true;
                    }
                    return self.func_raw_is_capturing_def(&args[0]);
                }
                false
            }
            HExprKind::Var(Binding::Func(c)) => self.def_is_capturing_fun(c.def),
            HExprKind::Var(Binding::Local(id)) => self
                .hir_local_name(*id)
                .and_then(|n| self.capturing_fun_locals.get(n).copied())
                .unwrap_or(false),
            _ => false,
        }
    }

    fn func_raw_is_capturing_def(&self, e: &HExpr) -> bool {
        match &e.kind {
            HExprKind::Cast(inner) => self.func_raw_is_capturing_def(inner),
            HExprKind::Var(Binding::Func(c)) => self.def_is_capturing_fun(c.def),
            _ => false,
        }
    }

    fn def_is_capturing_fun(&self, def: DefId) -> bool {
        let name = self.type_ctx.defs.name(def);
        self.closure_captures
            .get(name)
            .is_some_and(|caps| !caps.is_empty())
    }

    /// Records whether a fun-typed local's current value is capturing, for later JS-boundary checks.
    pub(in crate::semantics::analyzer) fn record_capturing_fun_local(
        &mut self,
        name: &str,
        ty: &Type,
        value: Option<&HExpr>,
    ) {
        if !matches!(ty, Type::Function(_, _)) {
            return;
        }
        let capturing = value.is_some_and(|v| self.func_expr_is_capturing(v));
        self.capturing_fun_locals
            .insert(name.to_string(), capturing);
    }

    /// Reports [`Self::JS_CAPTURING_CALLBACK_MSG`] and returns `false` when `e` is a capturing
    /// callback; otherwise returns `true`.
    pub(in crate::semantics::analyzer) fn ensure_captureless_js_callback(
        &self,
        e: &HExpr,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) -> bool {
        if self.func_expr_is_capturing(e) {
            diagnostics.report_error(Self::JS_CAPTURING_CALLBACK_MSG.to_string(), pos);
            false
        } else {
            true
        }
    }

    /// Builds a call to a `js` bridge extern (`js.__something`), resolved by its mangled def name.
    /// Returns `None` only if the bridge is somehow unregistered (a stdlib bug).
    fn js_bridge_call(&self, method: &str, args: Vec<HExpr>, ret: TypeId) -> Option<HExpr> {
        let mangled = method_fn(crate::mir::js_abi::JS_TYPE, method);
        let def = self.type_ctx.defs.lookup(DefKind::Function, &mangled)?;
        Some(HExpr::new(
            ret,
            HExprKind::Call {
                callee: Callee {
                    def,
                    instance: vec![],
                    ret,
                },
                args,
            },
        ))
    }

    /// Wraps `e` in an implicit cast to primitive `prim` (for widening a boxing argument to the
    /// bridge's declared parameter type, e.g. `float` -> `double`).
    fn cast_prim(&mut self, e: HExpr, prim: PrimTy) -> HExpr {
        let ty = self.type_ctx.interner.prim(prim);
        HExpr::new(ty, HExprKind::Cast(Box::new(e)))
    }

    /// Boxes a Dream value into a `js` handle: a `js` value passes through; primitives/`string` route
    /// through the matching `__box_*` bridge; a `fun(js): void` / `fun(): void` is wrapped as a JS
    /// callable. Any other type (struct/class/union/array/list) yields `None` (a compile error at the
    /// call site, pointing at `js.object()` / `js.array()`).
    ///
    /// A capturing `fun(...)` yields `None` after reporting via `diagnostics` when provided — the
    /// host bridges strip the closure env word, so only captureless functions are marshalable.
    pub(super) fn box_to_js(
        &mut self,
        e: HExpr,
        pos: Option<TextSpan>,
        diagnostics: Option<&mut DiagnosticBag>,
    ) -> Option<HExpr> {
        let js = self.type_ctx.interner.js();
        let stripped = e.ty;
        let kind = self.type_ctx.interner.kind(stripped).clone();
        match kind {
            TyKind::Js => Some(e),
            TyKind::Enum(_) => self.js_bridge_call("__box_int", vec![e], js),
            TyKind::Prim(p) => match p {
                PrimTy::String => self.js_bridge_call("__box_string", vec![e], js),
                PrimTy::Bool => self.js_bridge_call("__box_bool", vec![e], js),
                PrimTy::Double => self.js_bridge_call("__box_double", vec![e], js),
                PrimTy::Float => {
                    let d = self.cast_prim(e, PrimTy::Double);
                    self.js_bridge_call("__box_double", vec![d], js)
                }
                PrimTy::Long | PrimTy::ULong => self.js_bridge_call("__box_long", vec![e], js),
                PrimTy::Int => self.js_bridge_call("__box_int", vec![e], js),
                PrimTy::UInt | PrimTy::Byte | PrimTy::Char => {
                    let i = self.cast_prim(e, PrimTy::Int);
                    self.js_bridge_call("__box_int", vec![i], js)
                }
            },
            TyKind::Func(params, _ret) => {
                // A Dream function handed to a JS API as a persistent handle. `e` is a boxed
                // `fun(...)` value (see `hir_set_func_value`); the host has no env-restoring
                // prologue of its own, so only the funcidx half is meaningful — a *capturing*
                // lambda would lose its environment and is rejected at compile time. Arity 0/1 use
                // the documented `func0`/`func` convenience bridges; any higher arity routes through
                // the generalized `__funcN` bridge, which receives the raw funcref-table index plus
                // the parameter count and wraps it host-side as `fun(js, …): void`. Each parameter
                // is marshaled as a `js` handle and the result is discarded.
                if self.func_expr_is_capturing(&e) {
                    if let Some(diagnostics) = diagnostics {
                        diagnostics
                            .report_error(Self::JS_CAPTURING_CALLBACK_MSG.to_string(), pos);
                    }
                    return None;
                }
                let funcidx = self.hir_funcbox_funcidx(e)?;
                match params.len() {
                    0 => self.js_bridge_call("func0", vec![funcidx], js),
                    1 => self.js_bridge_call("func", vec![funcidx], js),
                    n => {
                        let arity =
                            HExpr::new(self.type_ctx.interner.int(), HExprKind::IntLit(n as i64));
                        self.js_bridge_call("__funcN", vec![funcidx, arity], js)
                    }
                }
            }
            // A struct/class deep-copies into a plain JS object; the backend generates a
            // `$<Type>_to_js` marshaler that the `Cast` dispatches to (see `mir/emit/js_marshal.rs`).
            TyKind::Struct(..) => Some(HExpr::new(js, HExprKind::Cast(Box::new(e)))),
            _ => None,
        }
    }

    /// Unboxes a `js` value into primitive/`string` `target`, via the matching `__as_*` bridge (plus
    /// a widening/narrowing cast when `target` is not the bridge's own result type). Used at typed
    /// boundaries by `coerce_to`.
    pub(super) fn unbox_from_js(&mut self, e: HExpr, target: TypeId) -> HExpr {
        let target_stripped = target;
        // A reference struct/class target reconstructs from the JS object's properties via the
        // generated `$js_to_<Type>` marshaler that the `Cast` dispatches to.
        if matches!(
            self.type_ctx.interner.kind(target_stripped),
            TyKind::Struct(..)
        ) && self.type_ctx.interner.is_reference(target_stripped)
        {
            return HExpr::new(target_stripped, HExprKind::Cast(Box::new(e)));
        }
        let TyKind::Prim(p) = self.type_ctx.interner.kind(target_stripped).clone() else {
            return e;
        };
        let int = self.type_ctx.interner.int();
        let double = self.type_ctx.interner.double();
        let bool_ty = self.type_ctx.interner.bool();
        let string = self.type_ctx.interner.string();
        let call = match p {
            PrimTy::String => self.js_bridge_call("__as_string", vec![e], string),
            PrimTy::Bool => self.js_bridge_call("__as_bool", vec![e], bool_ty),
            PrimTy::Double => self.js_bridge_call("__as_double", vec![e], double),
            PrimTy::Float => {
                let d = self.js_bridge_call("__as_double", vec![e], double);
                return d
                    .map(|d| HExpr::new(target_stripped, HExprKind::Cast(Box::new(d))))
                    .unwrap_or_else(|| HExpr::new(target_stripped, HExprKind::FloatLit(0.0)));
            }
            PrimTy::Int => self.js_bridge_call("__as_int", vec![e], int),
            PrimTy::UInt | PrimTy::Byte | PrimTy::Char | PrimTy::Long | PrimTy::ULong => {
                let i = self.js_bridge_call("__as_int", vec![e], int);
                return i
                    .map(|i| HExpr::new(target_stripped, HExprKind::Cast(Box::new(i))))
                    .unwrap_or_else(|| HExpr::new(target_stripped, HExprKind::IntLit(0)));
            }
        };
        call.unwrap_or_else(|| HExpr::new(target_stripped, HExprKind::IntLit(0)))
    }

    /// A `string` literal HExpr (for the dynamic member/method name).
    fn js_name_lit(&self, name: &str) -> HExpr {
        let string = self.type_ctx.interner.string();
        HExpr::new(string, HExprKind::StringLit(name.to_string()))
    }

    /// Prepares one argument for a shadow-stack `js` call *slot*: unlike [`box_to_js`], primitives
    /// are NOT boxed into handles (the host reads them straight out of the tagged slot); only a
    /// `float` is widened to `double` so its slot payload is an `f64`. `js`, `string`, primitive,
    /// `enum`, a `fun(js)`/`fun()` callback, and a primitive/`string`/`js` array are all accepted as
    /// they are; any other type returns `None` (a compile error pointing at `js.object()`/`js.array()`).
    ///
    /// Capturing callbacks are rejected by the caller ([`js_slot_args`]) before this runs.
    fn js_slot_arg(&mut self, e: HExpr) -> Option<HExpr> {
        let stripped = e.ty;
        let kind = self.type_ctx.interner.kind(stripped).clone();
        match kind {
            TyKind::Js | TyKind::Enum(_) => Some(e),
            TyKind::Prim(PrimTy::Float) => Some(self.cast_prim(e, PrimTy::Double)),
            TyKind::Prim(_) => Some(e),
            // A callback slot carries its arity in the slot `aux` word (see `js_abi::slot_desc`), so
            // the host wraps the funcref as `fun(js, …): void` with the right number of `js`
            // parameters. Any arity is marshalable through the slot buffer (env is stripped at emit).
            TyKind::Func(..) => Some(e),
            TyKind::Array(elem) => {
                let ek = self.type_ctx.interner.kind(elem).clone();
                match ek {
                    TyKind::Prim(_) | TyKind::Js | TyKind::Enum(_) => Some(e),
                    _ => None,
                }
            }
            // A struct/class argument deep-copies into a JS object handle (a JS slot).
            TyKind::Struct(..) => self.box_to_js(e, None, None),
            _ => None,
        }
    }

    /// Prepares every argument via [`js_slot_arg`], reporting a compile error and returning `None` on
    /// the first non-marshalable one.
    fn js_slot_args(
        &mut self,
        args: Vec<Option<HExpr>>,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) -> Option<Vec<HExpr>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            let arg = arg?;
            if matches!(self.type_ctx.interner.kind(arg.ty), TyKind::Func(..))
                && !self.ensure_captureless_js_callback(&arg, pos, diagnostics)
            {
                return None;
            }
            let arg_display =
                crate::types::display_name(&self.type_ctx.interner, &self.type_ctx.defs, arg.ty);
            match self.js_slot_arg(arg) {
                Some(a) => out.push(a),
                None => {
                    diagnostics.report_error(
                        format!(
                            "cannot pass a value of type '{}' to a js call; build a JS value with js.object() / js.array() and set its members natively",
                            arg_display
                        ),
                        pos,
                    );
                    return None;
                }
            }
        }
        Some(out)
    }

    /// Builds a `JsCall` HIR node targeting the `js.__call`/`js.__invoke` bridge import, whose
    /// arguments the backend marshals through the shadow stack. Returns `None` only if the bridge is
    /// somehow unregistered (a stdlib bug).
    fn js_call_node(
        &self,
        bridge: &str,
        target: HExpr,
        method: Option<HExpr>,
        args: Vec<HExpr>,
    ) -> Option<HExpr> {
        let js = self.type_ctx.interner.js();
        let mangled = method_fn(crate::mir::js_abi::JS_TYPE, bridge);
        let def = self.type_ctx.defs.lookup(DefKind::Function, &mangled)?;
        Some(HExpr::new(
            js,
            HExprKind::JsCall {
                callee: Callee {
                    def,
                    instance: vec![],
                    ret: js,
                },
                target: Box::new(target),
                method: method.map(Box::new),
                args,
            },
        ))
    }

    /// Analyzes a method call `recv.method(args)` on a `js` receiver. A method actually declared on
    /// `js` (the stdlib conversion/release helpers such as `to_int`, `is_null`, `release`) is
    /// dispatched normally; any other name binds dynamically at runtime via `__call`.
    pub(super) fn analyze_js_member_call(
        &mut self,
        recv: Option<HExpr>,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let mangled = method_fn(crate::mir::js_abi::JS_TYPE, &method.text);
        // Cloned up front (rather than re-looked-up below) because the argument analysis loop needs
        // `&mut self`, which would otherwise conflict with a borrow held from this lookup.
        let known_sig = self.function_table.get_function(&mangled).ok();

        let mut arg_hirs = Vec::with_capacity(params.len());
        for (i, param) in params.iter().enumerate() {
            let saved_expected = self.current_expected_type.take();
            if let Some(ref sig) = known_sig {
                self.current_expected_type = sig.parameter_types.get(i).cloned();
            }
            let _ =
                self.analyze_expression(param, ctx.parent_function, ctx.symbol_table, diagnostics)?;
            self.current_expected_type = saved_expected;
            arg_hirs.push(self.hir_take());
        }

        if let Some(sig) = known_sig {
            // Explicit `js.func` / `js.func0` / `js.__funcN` also strip the env word host-side —
            // reject capturing handlers here (they skip `box_to_js` / FUNC-slot checks).
            if matches!(method.text.as_str(), "func" | "func0" | "__funcN") {
                for arg in arg_hirs.iter().flatten() {
                    if matches!(self.type_ctx.interner.kind(arg.ty), TyKind::Func(..))
                        && !self.ensure_captureless_js_callback(
                            arg,
                            Some(method.position),
                            diagnostics,
                        )
                    {
                        self.hir_none();
                        return Ok(Type::Unknown);
                    }
                }
            }
            let ret = sig.return_type.clone().unwrap_or(Type::Void);
            self.hir_set_method_call(recv, &sig.name, arg_hirs, &ret);
            return Ok(ret);
        }

        self.desugar_js_call(
            recv,
            &method.text,
            arg_hirs,
            Some(method.position),
            diagnostics,
        );
        Ok(Self::js_type())
    }

    /// `recv.name` -> `js.__get(recv, "name")`. Sets the last-expression HIR.
    pub(super) fn desugar_js_get(&mut self, recv: Option<HExpr>, name: &str) {
        if !self.hir_active() {
            self.hir_none();
            return;
        }
        let js = self.type_ctx.interner.js();
        let name_lit = self.js_name_lit(name);
        let call = match recv {
            Some(recv) => self.js_bridge_call("__get", vec![recv, name_lit], js),
            None => None,
        };
        self.hir_set_last(call);
    }

    /// `recv.name = value` -> `js.__set(recv, "name", box(value))`. Emits a void statement.
    pub(super) fn desugar_js_set(
        &mut self,
        recv: Option<HExpr>,
        name: &str,
        value: Option<HExpr>,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !self.hir_active() {
            return;
        }
        let void = self.type_ctx.interner.void();
        let name_lit = self.js_name_lit(name);
        let (Some(recv), Some(value)) = (recv, value) else {
            self.hir_fail();
            return;
        };
        let Some(value) = self.box_to_js(value, pos, Some(diagnostics)) else {
            // `box_to_js` already reported the capturing-callback diagnostic when applicable.
            if !diagnostics.has_errors() {
                diagnostics.report_error(
                    "cannot assign this value to a js property; build a JS value with js.object() / js.array()".to_string(),
                    pos,
                );
            }
            self.hir_fail();
            return;
        };
        let call = self.js_bridge_call("__set", vec![recv, name_lit, value], void);
        self.hir_expr_stmt(call);
    }

    /// `recv.name(args...)` -> `js.__call(recv, "name", [box(args)...])`. Sets `hir.last`.
    pub(super) fn desugar_js_call(
        &mut self,
        recv: Option<HExpr>,
        name: &str,
        args: Vec<Option<HExpr>>,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !self.hir_active() {
            self.hir_none();
            return;
        }
        let name_lit = self.js_name_lit(name);
        let Some(recv) = recv else {
            self.hir_none();
            return;
        };
        let Some(args) = self.js_slot_args(args, pos, diagnostics) else {
            self.hir_none();
            return;
        };
        let call = self.js_call_node("__call", recv, Some(name_lit), args);
        self.hir_set_last(call);
    }

    /// `recv(args...)` -> `js.__invoke(recv, [box(args)...])`. Sets `hir.last`.
    pub(super) fn desugar_js_invoke(
        &mut self,
        recv: Option<HExpr>,
        args: Vec<Option<HExpr>>,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !self.hir_active() {
            self.hir_none();
            return;
        }
        let Some(recv) = recv else {
            self.hir_none();
            return;
        };
        let Some(args) = self.js_slot_args(args, pos, diagnostics) else {
            self.hir_none();
            return;
        };
        let call = self.js_call_node("__invoke", recv, None, args);
        self.hir_set_last(call);
    }

    /// `js.global` (the bare property, not the `js.global("name")` call) -> `globalThis`, so member
    /// access chains like `js.global.document` / `js.global.fetch(...)` bind against the JS global
    /// scope. Sets `hir.last`.
    pub(super) fn desugar_js_global_this(&mut self) {
        if !self.hir_active() {
            self.hir_none();
            return;
        }
        let js = self.type_ctx.interner.js();
        let call = self.js_bridge_call("__global_this", vec![], js);
        self.hir_set_last(call);
    }

    /// The AST type `Option<js>` (the result of awaiting a `js` Promise).
    pub(super) fn option_js_type() -> Type {
        Type::Struct(
            synthetic_token(TokenKind::IdentifierToken, "Option"),
            Some(vec![Self::js_type()]),
        )
    }

    /// `await <jsExpr>` -> `await js.__await(<jsExpr>)`. Builds the async wrapper call whose result is
    /// `Future<Option<js>>` (so the enclosing `await` unwraps it to `Option<js>` - `Some` on resolve,
    /// `None` on rejection), letting a JS Promise be awaited natively. Returns the
    /// `Future<Option<js>>`-typed call HIR (to hand to `hir_set_await`), or `None` if the inner
    /// expression was not representable.
    pub(super) fn desugar_js_await(&mut self, inner: Option<HExpr>) -> Option<HExpr> {
        let recv = inner?;
        let fut = self
            .type_ctx
            .lower(&Self::future_type(Self::option_js_type()));
        self.js_bridge_call("__await", vec![recv], fut)
    }

    /// `recv[key]` -> `js.__index_get(recv, box(key))`. Sets `hir.last`.
    pub(super) fn desugar_js_index_get(
        &mut self,
        recv: Option<HExpr>,
        key: Option<HExpr>,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !self.hir_active() {
            self.hir_none();
            return;
        }
        let js = self.type_ctx.interner.js();
        let (Some(recv), Some(key)) = (recv, key) else {
            self.hir_none();
            return;
        };
        let Some(key) = self.box_to_js(key, pos, Some(diagnostics)) else {
            if !diagnostics.has_errors() {
                diagnostics.report_error("cannot use this value as a js index key".to_string(), pos);
            }
            self.hir_none();
            return;
        };
        let call = self.js_bridge_call("__index_get", vec![recv, key], js);
        self.hir_set_last(call);
    }

    /// `recv[key] = value` -> `js.__index_set(recv, box(key), box(value))`. Emits a void statement.
    pub(super) fn desugar_js_index_set(
        &mut self,
        recv: Option<HExpr>,
        key: Option<HExpr>,
        value: Option<HExpr>,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !self.hir_active() {
            return;
        }
        let void = self.type_ctx.interner.void();
        let (Some(recv), Some(key), Some(value)) = (recv, key, value) else {
            self.hir_fail();
            return;
        };
        let key = self.box_to_js(key, pos, Some(diagnostics));
        let value = self.box_to_js(value, pos, Some(diagnostics));
        let (Some(key), Some(value)) = (key, value) else {
            if !diagnostics.has_errors() {
                diagnostics.report_error(
                    "cannot use this value as a js index key/value".to_string(),
                    pos,
                );
            }
            self.hir_fail();
            return;
        };
        let call = self.js_bridge_call("__index_set", vec![recv, key, value], void);
        self.hir_expr_stmt(call);
    }
}
