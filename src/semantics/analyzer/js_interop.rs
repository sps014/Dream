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
use crate::hir::{Callee, HExpr, HExprKind};
use crate::semantics::analyzer::Analyzer;
use crate::semantics::errors::SemanticError;
use crate::syntax::nodes::{ExpressionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::text::text_span::TextSpan;
use crate::types::{method_fn, DefKind, PrimTy, TyKind, TypeId};

impl<'a> Analyzer<'a> {
    /// The legacy AST `Type` for the dynamic `js` type (a bare nominal name the type context lowers
    /// to `TyKind::Js`).
    pub(super) fn js_type() -> Type {
        Type::Struct(synthetic_token(TokenKind::IdentifierToken, "js"), None)
    }

    /// True if `ty` is the dynamic `js` type.
    pub(super) fn is_js_type(&self, ty: &Type) -> bool {
        ty.get_type() == "js"
    }

    /// Builds a call to a `js` bridge extern (`js.__something`), resolved by its mangled def name.
    /// Returns `None` only if the bridge is somehow unregistered (a stdlib bug).
    fn js_bridge_call(&self, method: &str, args: Vec<HExpr>, ret: TypeId) -> Option<HExpr> {
        let mangled = method_fn("js", method);
        let def = self.type_ctx.defs.lookup(DefKind::Function, &mangled)?;
        Some(HExpr::new(
            ret,
            HExprKind::Call {
                callee: Callee { def, instance: vec![], ret },
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
    pub(super) fn box_to_js(&mut self, e: HExpr) -> Option<HExpr> {
        let js = self.type_ctx.interner.js();
        let stripped = self.type_ctx.interner.strip_nullable(e.ty);
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
                // A Dream function handed to a JS API. Canonical shapes are `fun(js): void`
                // (one JS argument, e.g. an event) and `fun(): void`.
                match params.len() {
                    1 => self.js_bridge_call("func", vec![e], js),
                    0 => self.js_bridge_call("func0", vec![e], js),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Unboxes a `js` value into primitive/`string` `target`, via the matching `__as_*` bridge (plus
    /// a widening/narrowing cast when `target` is not the bridge's own result type). Used at typed
    /// boundaries by `coerce_to`.
    pub(super) fn unbox_from_js(&mut self, e: HExpr, target: TypeId) -> HExpr {
        let target_stripped = self.type_ctx.interner.strip_nullable(target);
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

    /// Boxes each argument and collects them into a `js[]` array literal. Reports a compile error and
    /// returns `None` if any argument is not marshalable into `js`.
    fn js_args_array(
        &mut self,
        args: Vec<Option<HExpr>>,
        pos: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) -> Option<HExpr> {
        let js = self.type_ctx.interner.js();
        let arr_ty = self.type_ctx.interner.array(js);
        let mut boxed = Vec::with_capacity(args.len());
        for arg in args {
            let arg = arg?;
            let arg_display = crate::types::display_name(
                &self.type_ctx.interner,
                &self.type_ctx.defs,
                self.type_ctx.interner.strip_nullable(arg.ty),
            );
            match self.box_to_js(arg) {
                Some(b) => boxed.push(b),
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
        Some(HExpr::new(
            arr_ty,
            HExprKind::ArrayLit { elem_ty: js, elems: boxed },
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
        let mangled = method_fn("js", &method.text);
        let known = self.function_table.get_function(&mangled).is_ok();

        let mut arg_hirs = Vec::with_capacity(params.len());
        for param in params.iter() {
            let _ = self.analyze_expression(param, ctx.parent_function, ctx.symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
        }

        if known {
            let sig = self.function_table.get_function(&mangled).unwrap().clone();
            let ret = sig.return_type.clone().unwrap_or(Type::Void);
            self.hir_set_method_call(recv, &sig.name, arg_hirs, &ret);
            return Ok(ret);
        }

        self.desugar_js_call(recv, &method.text, arg_hirs, Some(method.position), diagnostics);
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
        let Some(value) = self.box_to_js(value) else {
            diagnostics.report_error(
                "cannot assign this value to a js property; build a JS value with js.object() / js.array()".to_string(),
                pos,
            );
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
        let js = self.type_ctx.interner.js();
        let name_lit = self.js_name_lit(name);
        let Some(recv) = recv else {
            self.hir_none();
            return;
        };
        let Some(arr) = self.js_args_array(args, pos, diagnostics) else {
            self.hir_none();
            return;
        };
        let call = self.js_bridge_call("__call", vec![recv, name_lit, arr], js);
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
        let js = self.type_ctx.interner.js();
        let Some(recv) = recv else {
            self.hir_none();
            return;
        };
        let Some(arr) = self.js_args_array(args, pos, diagnostics) else {
            self.hir_none();
            return;
        };
        let call = self.js_bridge_call("__invoke", vec![recv, arr], js);
        self.hir_set_last(call);
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
        let Some(key) = self.box_to_js(key) else {
            diagnostics.report_error("cannot use this value as a js index key".to_string(), pos);
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
        let (Some(key), Some(value)) = (self.box_to_js(key), self.box_to_js(value)) else {
            diagnostics.report_error("cannot use this value as a js index key/value".to_string(), pos);
            self.hir_fail();
            return;
        };
        let call = self.js_bridge_call("__index_set", vec![recv, key, value], void);
        self.hir_expr_stmt(call);
    }
}
