//! Analysis of call expressions: free-function and overload resolution, method calls, static /
//! namespaced calls (`Math.*` / `JSON.*` / async intrinsics / `derive` helpers), and constructors.

use super::super::*;
use crate::diagnostics::DiagnosticBag;
use crate::intrinsics;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::FunctionTableInfo;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::{
    canonical_type_name, is_numeric_primitive, is_unknown_type_name, mangle_generic, strip_nullable,
};
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::types::method_fn;
use std::cell::RefCell;
use std::rc::Rc;

/// Outcome of looking up an indexer/enumerator "hook" method (`get`/`set`/`iterator`/`next`) on a
/// struct receiver, for the desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.
// One variant carries a resolved method descriptor while others are unit-like; the value is
// short-lived and never stored en masse, so the size spread does not warrant boxing.
#[allow(clippy::large_enum_variant)]
enum HookResolution {
    /// The receiver is not a struct, or it has no method with that name: the sugar is unavailable.
    Absent,
    /// A method with that name exists but cannot serve as a hook; carries a human-readable reason
    /// (it is static, async, or has the wrong number of parameters).
    Ineligible(String),
    /// A usable hook: an accessible instance, non-async method with the requested declared arity.
    Eligible(FunctionTableInfo),
}

impl<'a> Analyzer<'a> {
    pub(crate) fn analyze_method_call(
        &mut self,
        obj: &ExpressionNode<'a>,
        method: &SyntaxToken,
        _generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        if let ExpressionNode::Identifier(id) = obj {
            if let Some(t) =
                self.try_analyze_static_method(id, method, _generic_args, params, ctx, diagnostics)?
            {
                return Ok(t);
            }
        }

        let obj_type =
            self.analyze_expression(obj, ctx.parent_function, ctx.symbol_table, diagnostics)?;
        let obj_hir = self.hir_take();

        // The receiver was already poisoned by an earlier error; still type-check the arguments
        // (to surface their own mistakes) but stay quiet about the method itself and stay poison.
        if obj_type.is_unknown() {
            for param in params.iter() {
                let _ = self.analyze_expression(
                    param,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                );
            }
            self.hir_none();
            return Ok(Type::Unknown);
        }

        // A `js`-typed receiver binds dynamically: a declared `js` helper (e.g. `to_int`) dispatches
        // normally, any other name is a runtime JS method call. This must run before the builtin/
        // instance resolution below, which has no notion of dynamic members.
        if self.is_js_type(&obj_type) {
            return self.analyze_js_member_call(obj_hir, method, params, ctx, diagnostics);
        }

        // Builtin methods: `size()` lowers to `ArrayLen`; the rest (`to_string`/`char_at`/`hash_code`)
        // need runtime defs and stay on the legacy path (they clear HIR inside the helper). The
        // receiver is threaded in so `len` can wrap it; it is left intact when no builtin matches.
        let mut recv = obj_hir;
        if let Some(t) =
            self.analyze_builtin_method(&obj_type, method, params, ctx, &mut recv, diagnostics)?
        {
            return Ok(t);
        }

        self.analyze_instance_method(&obj_type, method, params, ctx, recv, diagnostics)
    }

    /// Handles `Type.method(args)` static dispatch when the receiver `id` names a type rather than
    /// a local: discriminated-union variant construction, on-the-fly monomorphization of generic
    /// static methods (including the `Buffer.alloc` and promise-combinator intrinsics), and plain
    /// static-method resolution. Returns `Ok(Some(type))` when handled, `Ok(None)` when `id` is a
    /// local or names no static member (so the caller falls through to instance dispatch).
    pub(crate) fn try_analyze_static_method(
        &mut self,
        id: &SyntaxToken,
        method: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Option<Type>, SemanticError> {
        // The receiver names a type (not a local variable), so resolve `{type}_{method}` directly
        // with no implicit `this`.
        let is_local = (*ctx.symbol_table).as_ref().borrow().get_symbol(id).is_ok();
        if is_local {
            return Ok(None);
        }

        // `Enum.Variant(args)`: construct a discriminated-union value.
        if let Some(t) = self.analyze_variant_construction(
            &id.text,
            method,
            params,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )? {
            return Ok(Some(t));
        }

        let type_name = canonical_type_name(&id.text)
            .unwrap_or(id.text.as_str())
            .to_string();
        let base = method_fn(&type_name, &method.text);

        // File/module-level visibility (Axis 2): reaching a static member requires the type itself
        // to be visible. A non-public class/struct is only referenceable from its declaring file.
        if let Some(info) = self.struct_table.get_struct(&type_name) {
            if !self.visible_across_files(
                &info.file_path,
                info.is_public,
                ctx.parent_function.file_path.as_ref(),
            ) {
                let decl_file = info.file_path.clone();
                self.report_not_public("Type", &type_name, &decl_file, id.position, diagnostics);
            }
        } else if let Some(template) = self.generic_structs.get(&type_name) {
            let (decl_file, is_public) = (template.file_path.clone(), template.is_public);
            if !self.visible_across_files(
                &decl_file,
                is_public,
                ctx.parent_function.file_path.as_ref(),
            ) {
                self.report_not_public("Type", &type_name, &decl_file, id.position, diagnostics);
            }
        } else {
            self.check_type_visible(
                &type_name,
                ctx.parent_function.file_path.as_ref(),
                id.position,
                diagnostics,
            );
        }

        // A static method on a *generic class* (`Cache<int>.make(...)`). The class's type arguments
        // ride on the call's generic-argument slot (attached by the parser). Monomorphize the class
        // so its concrete static methods (`Cache_int_make`, ...) are registered, then dispatch the
        // concrete method through the normal static-call path (which enforces class-level privacy).
        if self.generic_structs.contains_key(&type_name) {
            let args: Vec<Type> = match generic_args {
                Some(a) if !a.is_empty() => a
                    .iter()
                    .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings))
                    .collect(),
                _ => {
                    diagnostics.report_error(
                        format!(
                            "Generic class '{}' requires type arguments to call a static method, e.g. {}<int>.{}(...)",
                            type_name, type_name, method.text
                        ),
                        Some(id.position),
                    );
                    return Ok(Some(Type::Unknown));
                }
            };
            self.ensure_struct_instantiated(&type_name, &args, &id.position, diagnostics);
            let mangled_type = mangle_generic(&type_name, &args);
            let ret = self.analyze_static_call(
                &mangled_type,
                method,
                params,
                ctx.parent_function,
                ctx.symbol_table,
                diagnostics,
            )?;
            return Ok(Some(ret));
        }

        // Support generic static method calls by monomorphizing them on the fly.
        if self.generic_functions.contains_key(&base) {
            let template = *self.generic_functions.get(&base).unwrap();
            let mut params_types = vec![];
            let mut arg_hirs = vec![];
            for param in params.iter() {
                let t = self.analyze_expression(
                    param,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                )?;
                arg_hirs.push(self.hir_take());
                params_types.push(t.get_type());
            }
            // `System.print`/`println` are generic builtins (not real monomorphizations): they lower
            // to the host `print_*` imports, so handle them before the generic-instance machinery.
            if let Some(op @ (intrinsics::IntrinsicOp::Print | intrinsics::IntrinsicOp::Println)) =
                intrinsics::IntrinsicOp::from_attributes(&template.attributes)
            {
                if params.len() != 1 {
                    diagnostics.report_error(
                        format!(
                            "'{}' expects exactly 1 argument, got {}",
                            method.text,
                            params.len()
                        ),
                        Some(method.position),
                    );
                    self.hir_none();
                } else {
                    let newline = op == intrinsics::IntrinsicOp::Println;
                    self.hir_set_print(arg_hirs.into_iter().next().flatten(), newline);
                }
                return Ok(Some(Type::Unknown));
            }
            // Generic static calls / intrinsics need an `InstanceId` (a later slice); stay out of
            // HIR coverage regardless of which sub-branch handles the call.
            self.hir_none();
            // `Buffer.alloc<T>(len)`: a generic intrinsic that allocates a zero-initialized
            // `T[]`. The element type comes from the explicit type argument (resolved
            // through the active monomorphization bindings so `Buffer.alloc<T>` inside a
            // `List<int>` method yields `int[]`).
            if intrinsics::IntrinsicOp::from_attributes(&template.attributes)
                == Some(intrinsics::IntrinsicOp::ArrayNew)
            {
                let element = match generic_args.as_ref().and_then(|g| g.first()) {
                    Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                    None => {
                        diagnostics.report_error(
                            "'Buffer.alloc' requires a type argument, e.g. Buffer.alloc<int>(n)"
                                .to_string(),
                            Some(method.position),
                        );
                        Type::Void
                    }
                };
                if params_types.len() != 1 {
                    diagnostics.report_error(
                        format!(
                            "'Buffer.alloc' expects exactly 1 argument (length), got {}",
                            params_types.len()
                        ),
                        Some(method.position),
                    );
                } else if params_types[0] != "int" && !is_unknown_type_name(&params_types[0]) {
                    diagnostics.report_error(
                        format!("'Buffer.alloc' length must be int, got {}", params_types[0]),
                        Some(method.position),
                    );
                }
                self.hir_set_array_new(&element, arg_hirs.into_iter().next().flatten());
                return Ok(Some(Type::Array(Box::new(element))));
            }

            // `Bytes.of<T>(v)` / `Bytes.to<T>(bytes)`: raw byte-copy conversions between a blittable
            // value and a `byte[]` buffer (used by the worker-boundary adapter). `of` copies the
            // value's bytes out to a fresh buffer; `to` reconstructs a `T` from a buffer.
            let byte_op = intrinsics::IntrinsicOp::from_attributes(&template.attributes);
            if byte_op == Some(intrinsics::IntrinsicOp::ToBytes) {
                let named = |name: &str| -> Type {
                    let mut t = method.clone();
                    t.text = name.to_string();
                    Type::from_token(t).unwrap_or(Type::Unknown)
                };
                if params_types.len() != 1 {
                    diagnostics.report_error(
                        format!(
                            "'Bytes.of' expects exactly 1 argument (the value), got {}",
                            params_types.len()
                        ),
                        Some(method.position),
                    );
                }
                let payload = match generic_args.as_ref().and_then(|g| g.first()) {
                    Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                    None => params_types.first().map(|s| named(s)).unwrap_or(Type::Unknown),
                };
                self.require_unmanaged(&payload, "Bytes.of", &method.position, diagnostics);
                self.hir_set_to_bytes(arg_hirs.into_iter().next().flatten());
                return Ok(Some(Type::Array(Box::new(named("byte")))));
            }
            if byte_op == Some(intrinsics::IntrinsicOp::FromBytes) {
                let target = match generic_args.as_ref().and_then(|g| g.first()) {
                    Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                    None => {
                        diagnostics.report_error(
                            "'Bytes.to' requires a type argument, e.g. Bytes.to<Point>(bytes)"
                                .to_string(),
                            Some(method.position),
                        );
                        Type::Void
                    }
                };
                self.require_unmanaged(&target, "Bytes.to", &method.position, diagnostics);
                self.hir_set_from_bytes(&target, arg_hirs.into_iter().next().flatten());
                return Ok(Some(target));
            }

            let bindings = self.infer_generic_bindings(
                template,
                generic_args,
                &params_types,
                &method.position,
                diagnostics,
            );
            let mangled_name = mangle_bindings(&base, &bindings);

            // Promise combinators (`Promise.all/any/race`) are typed by the shared async
            // intrinsic logic; classify via the registry and delegate when applicable.
            if let Some(combinator) = intrinsics::IntrinsicOp::from_attributes(&template.attributes)
                .and_then(|op| op.promise_combinator())
            {
                let mut s_tok = method.clone();
                s_tok.text = combinator.to_string();
                let ret = self.analyze_async_intrinsic(
                    &s_tok,
                    params,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                )?;
                // `analyze_async_intrinsic` only types the combinator; its argument analysis leaves
                // the future-array HIR in `last`. Reuse it as the single arg of a direct call to the
                // combinator intrinsic so the MIR backend lowers it to `$dream_all/$dream_any`
                // (rather than emitting only the array, which would await the raw array pointer).
                let arg_hir = self.hir_take();
                self.hir_set_call(&base, vec![arg_hir], &ret);
                return Ok(Some(ret));
            }

            // `JSON.serialize<T>(v)` / `JSON.deserialize<T>(text)`: the `@json` derive emits
            // `<T>.to_json()` / `<T>.from_json()` (see `driver::json_derive`), and `JSON.stringify` /
            // `JSON.parse` are ordinary static methods. Expand the intrinsic into that composition so
            // the whole thing lowers through MIR (rather than staying on the legacy expansion).
            let json_op = intrinsics::IntrinsicOp::from_attributes(&template.attributes);
            if json_op == Some(intrinsics::IntrinsicOp::JsonSerialize) {
                let named = |name: &str| -> Type {
                    let mut t = method.clone();
                    t.text = name.to_string();
                    Type::from_token(t).unwrap_or(Type::Unknown)
                };
                let struct_name = params_types
                    .first()
                    .map(|s| s.trim_end_matches('?').to_string())
                    .unwrap_or_default();
                let value = arg_hirs.into_iter().next().flatten();
                // `<T>.to_json(value)` (a `this`-taking method, called free with the receiver as arg0).
                self.hir_set_call(
                    &method_fn(&struct_name, "to_json"),
                    vec![value],
                    &named("JsonValue"),
                );
                let to_json = self.hir_take();
                self.hir_set_call("JSON_stringify", vec![to_json], &named("string"));
                return Ok(Some(named("string")));
            }
            if json_op == Some(intrinsics::IntrinsicOp::JsonDeserialize) {
                let named = |name: &str| -> Type {
                    let mut t = method.clone();
                    t.text = name.to_string();
                    Type::from_token(t).unwrap_or(Type::Unknown)
                };
                let t_type = match generic_args.as_ref().and_then(|g| g.first()) {
                    Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                    None => {
                        diagnostics.report_error(
                            "'JSON.deserialize' requires a type argument, e.g. JSON.deserialize<T>(text)"
                                .to_string(),
                            Some(method.position),
                        );
                        Type::Void
                    }
                };
                let struct_name = t_type.get_type().trim_end_matches('?').to_string();
                let text = arg_hirs.into_iter().next().flatten();
                self.hir_set_call("JSON_parse", vec![text], &named("JsonValue"));
                let parsed = self.hir_take();
                self.hir_set_call(&method_fn(&struct_name, "from_json"), vec![parsed], &t_type);
                return Ok(Some(t_type));
            }

            // Class-level privacy (Axis 1): a non-public generic static method is private to its
            // declaring type, exactly like the non-generic path in `analyze_static_call`. Without
            // this the generic branch below would return early and skip the check entirely.
            if !template.is_public && !self.in_methods_of(ctx.parent_function, &type_name) {
                diagnostics.report_error(
                    format!("'{}' is private to '{}'", method.text, type_name),
                    Some(method.position),
                );
            }

            if self.function_table.get_function(&mangled_name).is_err() {
                let mut specialized = template.clone();
                Self::substitute_generic_signature(&mut specialized, &bindings);
                let specialized_ref: &'a FunctionNode<'a> = self.arena.alloc(specialized);
                let info = FunctionTableInfo::from(specialized_ref);
                let _ = self.function_table.add_function(mangled_name.clone(), info);
                self.instantiated_generics
                    .insert(mangled_name.clone(), (bindings, specialized_ref));
            }
            let info = match self.function_table.get_function(&mangled_name) {
                Ok(info) => info,
                Err(_) => {
                    diagnostics.report_error(
                        format!("Function '{}' could not be instantiated", mangled_name),
                        Some(method.position),
                    );
                    return Ok(Some(Type::Unknown));
                }
            };
            if info.is_async {
                return Ok(Some(Type::Unknown));
            }
            return Ok(Some(Type::Unknown));
        }

        if self.function_table.is_overloaded(&base)
            || self.function_table.get_function(&base).is_ok()
        {
            return Ok(Some(self.analyze_static_call(
                &type_name,
                method,
                params,
                ctx.parent_function,
                ctx.symbol_table,
                diagnostics,
            )?));
        }

        Ok(None)
    }

    /// Type-checks the builtin methods available on every (or every primitive/array) receiver:
    /// `size()`, `str.char_at(i)`, and the `to_string`/`hash_code` object protocol (a C-style enum's
    /// `to_string()` renders its variant name). Returns `Ok(Some(result_type))` when the call is a
    /// builtin (so the caller returns it) or `Ok(None)` to fall through to normal instance-method
    /// dispatch. A user-defined `to_string`/`hash_code` override yields `None` so the override is
    /// dispatched normally.
    pub(crate) fn analyze_builtin_method(
        &mut self,
        obj_type: &Type,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        receiver: &mut Option<crate::hir::HExpr>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Option<Type>, SemanticError> {
        // Default: no builtin HIR. `len` opts back in below; the others stay on the legacy path.
        self.hir_none();

        // `arr.size()` / `str.size()`: built-in element-count method on arrays and strings (the same
        // `size()` the stdlib `List`/`Map` expose, so every collection is queried the same way).
        if method.text == intrinsics::SIZE {
            let base = strip_nullable(&obj_type.get_type()).to_string();
            if base.ends_with("[]") || base == "string" {
                if !params.is_empty() {
                    diagnostics.report_error(
                        format!("'size' takes no arguments, got {}", params.len()),
                        Some(method.position),
                    );
                }
                self.hir_set_array_len(receiver.take());
                return Ok(Some(Type::Integer(synthetic_token(
                    TokenKind::DataTypeToken,
                    "int",
                ))));
            }
        }

        // `str.char_at(i)`: built-in character accessor on strings (low-level read).
        if method.text == intrinsics::CHAR_AT && strip_nullable(&obj_type.get_type()) == "string" {
            if params.len() != 1 {
                diagnostics.report_error(
                    format!(
                        "'char_at' expects exactly 1 argument (index), got {}",
                        params.len()
                    ),
                    Some(method.position),
                );
            }
            let mut idx_hir: Option<crate::hir::HExpr> = None;
            for param in params.iter() {
                let pt = self.analyze_expression(
                    param,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                )?;
                idx_hir = self.hir_take();
                if !pt.is_int() && !pt.is_unknown() {
                    diagnostics.report_error(
                        format!("'char_at' index must be int, got {}", pt.get_type()),
                        param.position(),
                    );
                }
            }
            self.hir_set_char_at(receiver.take(), idx_hir);
            return Ok(Some(Type::Char(synthetic_token(
                TokenKind::DataTypeToken,
                "char",
            ))));
        }

        // Object protocol: `x.to_string()` / `x.hash_code()` are available on every type. A
        // user-defined override (registered as `{Type}_to_string`) takes precedence and is resolved
        // by the normal method lookup below; otherwise fall back to the builtin protocol.
        if method.text == intrinsics::TO_STRING || method.text == intrinsics::HASH_CODE {
            let receiver_name = match Self::resolve_struct_parts(obj_type) {
                Some((base_name, generic_args)) => mangle_generic(&base_name, &generic_args),
                None => strip_nullable(&obj_type.get_type()).to_string(),
            };
            let user_method = method_fn(&receiver_name, &method.text);
            let has_override = self.function_table.is_overloaded(&user_method)
                || self.function_table.get_function(&user_method).is_ok();
            if !has_override {
                if !params.is_empty() {
                    diagnostics.report_error(
                        format!("'{}' takes no arguments, got {}", method.text, params.len()),
                        Some(method.position),
                    );
                }
                if method.text == intrinsics::TO_STRING {
                    // A C-style enum's `to_string()` renders the variant name (e.g. `Color.Green`
                    // -> "Green") by mapping the discriminant to its interned name, rather than the
                    // generic object protocol (which would stringify the underlying integer).
                    if let Some(members) = self.enum_table.get(&receiver_name) {
                        let arms: Vec<(i64, String)> = members
                            .iter()
                            .map(|(name, value)| (*value as i64, name.clone()))
                            .collect();
                        self.hir_set_enum_name(receiver.take(), arms);
                        return Ok(Some(Type::String(synthetic_token(
                            TokenKind::DataTypeToken,
                            "string",
                        ))));
                    }
                    self.hir_set_to_string(receiver.take());
                    return Ok(Some(Type::String(synthetic_token(
                        TokenKind::DataTypeToken,
                        "string",
                    ))));
                }
                self.hir_set_hash_code(receiver.take());
                return Ok(Some(Type::Integer(synthetic_token(
                    TokenKind::DataTypeToken,
                    "int",
                ))));
            }
        }

        Ok(None)
    }

    /// Resolves and type-checks an instance method call `obj.method(args)` once the receiver type
    /// (`obj_type`) is known and the builtins/static cases have been ruled out: monomorphizes the
    /// receiver, selects the (possibly overloaded) `{Type}_{method}`, enforces privacy and the
    /// argument arity/types, and returns the call's result type (a `Future<T>` for `async`).
    /// If `obj_type` (ignoring any nullable wrapper) names an interface, returns that interface's
    /// name; otherwise `None`.
    pub(crate) fn interface_receiver_name(&self, obj_type: &Type) -> Option<String> {
        let name = strip_nullable(&obj_type.get_type()).to_string();
        if self.is_interface_name(&name) {
            Some(name)
        } else {
            None
        }
    }

    /// Dispatches a method call on an interface-typed receiver. Resolves `method` against the
    /// interface's ordered signature list (yielding its local slot index and return type),
    /// type-checks the arguments, and emits a dynamically-dispatched `InterfaceCall` HIR node.
    pub(crate) fn analyze_interface_method(
        &mut self,
        iface_name: &str,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        receiver: Option<crate::hir::HExpr>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        for param in params.iter() {
            let t =
                self.analyze_expression(param, ctx.parent_function, ctx.symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
            arg_types.push(t.get_type());
        }

        let methods = self
            .interface_methods
            .get(iface_name)
            .cloned()
            .unwrap_or_default();
        let Some((slot, im)) = methods
            .iter()
            .enumerate()
            .find(|(_, m)| m.name.text == method.text)
        else {
            return Err(report(
                diagnostics,
                format!("interface '{}' has no method '{}'", iface_name, method.text),
                Some(method.position),
            ));
        };

        let expected: Vec<String> = im.parameters.iter().map(|p| p.type_.get_type()).collect();
        // Calling an `async` interface method is eager and yields a `Future<T>` handle (just like an
        // async instance method); the concrete implementation dispatches to a `Future`-producing
        // constructor. The caller must `await` the result.
        let base_ret = im.return_type.clone().unwrap_or(Type::Void);
        let ret_type = if im.is_async {
            Self::future_type(base_ret)
        } else {
            base_ret
        };
        if expected.len() != arg_types.len() {
            diagnostics.report_error(
                format!(
                    "interface method '{}.{}' expects {} arguments, got {}",
                    iface_name,
                    method.text,
                    expected.len(),
                    arg_types.len()
                ),
                Some(method.position),
            );
            self.hir_none();
            return Ok(ret_type);
        }
        for (i, given) in arg_types.iter().enumerate() {
            if !self.type_str_assignable(&expected[i], given) {
                diagnostics.report_error(
                    format!(
                        "interface method '{}.{}' expects parameter {} to be {}, got {}",
                        iface_name,
                        method.text,
                        i + 1,
                        expected[i],
                        given
                    ),
                    Some(method.position),
                );
            }
        }

        let iface_id = self.interface_methods.get_index_of(iface_name).unwrap_or(0);
        // The `call_indirect` signature is `fun(this, params...): ret`, with `this` typed as
        // `object` (an `i32` pointer, matching every concrete implementation's receiver).
        let sig = self.interface_dispatch_sig(im);
        self.hir_set_interface_call(receiver, iface_id, slot, sig, arg_hirs, &ret_type);
        Ok(ret_type)
    }

    /// Interns the `fun(this, params...): ret` function type used to `call_indirect` an interface
    /// method: `this` is `object` (a tagged pointer), followed by the method's declared parameters
    /// and its return type. The same signature is used to declare the WASM `call_indirect` type.
    pub(crate) fn interface_dispatch_sig(
        &mut self,
        method: &FunctionNode<'a>,
    ) -> crate::types::TypeId {
        let mut params = vec![self.type_ctx.interner.object()];
        for p in &method.parameters {
            let id = self.type_ctx.lower(&p.type_);
            params.push(id);
        }
        // An `async` interface method dispatches to a concrete async constructor whose WASM result
        // is the `Future` frame pointer (an `i32`), so the `call_indirect` signature returns an
        // `object`-shaped pointer regardless of the method's declared return type.
        let ret = if method.is_async {
            self.type_ctx.interner.object()
        } else {
            match &method.return_type {
                Some(t) => self.type_ctx.lower(t),
                None => self.type_ctx.interner.void(),
            }
        };
        self.type_ctx.interner.func(params, ret)
    }

    pub(crate) fn analyze_instance_method(
        &mut self,
        obj_type: &Type,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        receiver: Option<crate::hir::HExpr>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        // A generic interface receiver (e.g. `Container<int>`) must be monomorphized before dispatch
        // so its concrete method slots exist, even if no implementing class was instantiated earlier
        // in analysis order.
        if let Some((base, args)) = Self::resolve_struct_parts(obj_type) {
            if !args.is_empty() && self.is_generic_interface(&base) {
                self.ensure_interface_instantiated(&base, &args, &method.position, diagnostics);
            }
        }
        // Interface-typed receiver: the concrete implementation is unknown statically, so dispatch
        // dynamically through the interface's method table rather than resolving a static method.
        if let Some(iface_name) = self.interface_receiver_name(obj_type) {
            return self.analyze_interface_method(
                &iface_name,
                method,
                params,
                ctx,
                receiver,
                diagnostics,
            );
        }

        // Struct receivers are monomorphized to their concrete type name; primitive/`object`
        // receivers (which can carry methods via `extend`) use their canonical type name directly.
        let struct_name = match Self::resolve_struct_parts(obj_type) {
            Some((base_name, generic_args)) => {
                // A generic union receiver (e.g. `Option<int>`) is instantiated through the union
                // path so its extension methods are registered; everything else is a struct.
                self.ensure_type_instantiated(
                    &base_name,
                    &generic_args,
                    &method.position,
                    diagnostics,
                );
                mangle_generic(&base_name, &generic_args)
            }
            None => strip_nullable(&obj_type.get_type()).to_string(),
        };

        let mangled_name = method_fn(&struct_name, &method.text);

        // Analyze the explicit arguments once, then resolve the method (overloaded methods select
        // by argument types, with the receiver supplied as the implicit `this` argument).
        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        for param in params.iter() {
            let t =
                self.analyze_expression(param, ctx.parent_function, ctx.symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
            arg_types.push(t.get_type());
        }

        let store_sig = if self.function_table.is_overloaded(&mangled_name) {
            let mut selection_args = Vec::with_capacity(arg_types.len() + 1);
            selection_args.push(struct_name.clone());
            selection_args.extend(arg_types.iter().cloned());
            match self.select_function_overload(&mangled_name, &selection_args) {
                Ok(sig) => sig,
                Err(message) => {
                    return Err(report(diagnostics, message, Some(method.position)));
                }
            }
        } else {
            match self.function_table.get_function(&mangled_name) {
                Ok(s) => s.clone(),
                Err(_) => {
                    return Err(report(
                        diagnostics,
                        format!("Type '{}' has no method '{}'", struct_name, method.text),
                        Some(method.position),
                    ));
                }
            }
        };

        // Private methods (the default) may only be called from within the declaring type's own
        // methods; `public` exposes them to outside code.
        if !store_sig.is_public {
            let base_name = Self::resolve_struct_parts(obj_type)
                .map(|(b, _)| b)
                .unwrap_or_else(|| strip_nullable(&obj_type.get_type()).to_string());
            if !self.in_methods_of(ctx.parent_function, &base_name) {
                diagnostics.report_error(
                    format!("'{}' is private to '{}'", method.text, base_name),
                    Some(method.position),
                );
            }
        }

        let mut expected_params = store_sig.parameters.clone();
        let mut expected_defaults = store_sig.defaults.clone();

        // Remove 'this' from the expected params check since we supply it implicitly
        if !expected_params.is_empty() {
            expected_params.remove(0);
        }
        if !expected_defaults.is_empty() {
            expected_defaults.remove(0);
        }

        let required = expected_defaults
            .iter()
            .position(|d| d.is_some())
            .unwrap_or(expected_params.len());
        let total = expected_params.len();
        let given = arg_types.len();
        if given < required || given > total {
            let message = if required == total {
                format!(
                    "function {} expects {} parameters, got {}",
                    mangled_name, total, given
                )
            } else {
                format!(
                    "function {} expects between {} and {} parameters, got {}",
                    mangled_name, required, total, given
                )
            };
            diagnostics.report_error(message, Some(method.position));
            self.hir_none();
            return Ok(Type::Unknown);
        }

        // Fill omitted trailing arguments with their default values before type-checking/emit.
        self.substitute_default_args(
            &expected_defaults,
            &mut arg_types,
            &mut arg_hirs,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;

        self.validate_arguments(
            &format!("function {}", mangled_name),
            &expected_params,
            &arg_types,
            method.position,
            diagnostics,
        );

        // Calling an `async` method is eager and yields a `Future<T>` handle (like free async
        // functions); `await` retrieves the `T`.
        // An `async` method yields a `Future<T>` handle (carried by the `MethodCall`); `await`
        // unwraps it.
        let ret_type = if store_sig.is_async {
            Self::future_type(store_sig.return_type.unwrap_or(Type::Void))
        } else {
            store_sig.return_type.unwrap_or(Type::Void)
        };
        // Overloaded methods each register a distinct `DefId` under their emitted (signature-mangled)
        // name; resolve to the selected overload's name so the call targets the right instance.
        // Non-overloaded methods keep their base-mangled name.
        self.hir_set_method_call(receiver, &store_sig.name, arg_hirs, &ret_type);
        Ok(ret_type)
    }

    /// True when `parent_function` is a method whose implicit `this` receiver has base type
    /// `base_name` (allowing for monomorphized generic variants). Used to gate access to
    /// `_`-prefixed (private) members.
    pub(crate) fn in_methods_of(
        &self,
        parent_function: &FunctionNode<'a>,
        base_name: &str,
    ) -> bool {
        // A `static` method belongs to its declaring type, so it may access that type's private
        // members even though it has no `this` receiver. Static methods are registered under the
        // mangled name `{Type}_{method}`, so a name prefixed with `{base_name}_` identifies one.
        if parent_function.is_static {
            let name = &parent_function.name.text;
            return name == base_name
                || name.starts_with(&format!("{}_", base_name))
                || base_name.starts_with(&format!("{}_", name));
        }
        let Some(first) = parent_function.parameters.first() else {
            return false;
        };
        if first.name.text != "this" {
            return false;
        }
        let this_base = Self::resolve_struct_parts(&first.type_)
            .map(|(b, _)| b)
            .unwrap_or_else(|| strip_nullable(&first.type_.get_type()).to_string());
        this_base == base_name
            || this_base.starts_with(&format!("{}_", base_name))
            || base_name.starts_with(&format!("{}_", this_base))
    }

    /// Resolves a hook method named `method_name` (with declared arity `declared_arity`, i.e.
    /// excluding the implicit `this`) on struct receiver `obj_type`, ensuring the receiver's generic
    /// instance is registered first. Return-type shape checks (non-void for `get`, `Option<T>` for
    /// `next`, etc.) are left to the caller. An overloaded hook resolves to the first overload that
    /// matches the requested arity. A same-named method that is `static`, `async`, or of the wrong
    /// arity yields `Ineligible` (so `obj[i]`/`for..in` never silently hijack an ordinary method),
    /// while a call like `obj.get(i)` keeps resolving through the normal method path.
    fn resolve_hook_method(
        &mut self,
        obj_type: &Type,
        method_name: &str,
        declared_arity: usize,
        diagnostics: &mut DiagnosticBag,
    ) -> HookResolution {
        let (base_name, generic_args) = match Self::resolve_struct_parts(obj_type) {
            Some(parts) => {
                self.ensure_type_instantiated(&parts.0, &parts.1, &empty_span(), diagnostics);
                parts
            }
            // `string` is a built-in reference type carrying `extend string` methods (registered
            // under the `string` type name), so its `get`/`iterator` hooks resolve exactly like a
            // class's — no instantiation needed since `string` is not generic.
            None if matches!(obj_type, Type::String(_)) => ("string".to_string(), Vec::new()),
            None => return HookResolution::Absent,
        };
        let mono_name = mangle_generic(&base_name, &generic_args);
        let mangled = method_fn(&mono_name, method_name);

        let candidates: Vec<FunctionTableInfo> = if self.function_table.is_overloaded(&mangled) {
            self.function_table
                .overloads
                .get(&mangled)
                .map(|keys| {
                    keys.iter()
                        .filter_map(|k| self.function_table.get_function(k).ok())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            match self.function_table.get_function(&mangled) {
                Ok(info) => vec![info],
                Err(_) => return HookResolution::Absent,
            }
        };

        // Prefer an eligible candidate; otherwise remember why the first candidate was unusable.
        let mut ineligible_reason: Option<String> = None;
        for info in candidates {
            if info.is_static {
                ineligible_reason.get_or_insert_with(|| {
                    format!("'{}' must be a non-static instance method", method_name)
                });
                continue;
            }
            if info.is_async {
                ineligible_reason
                    .get_or_insert_with(|| format!("'{}' cannot be async", method_name));
                continue;
            }
            // Instance methods carry an implicit `this` at parameter index 0.
            let declared = info.parameters.len().saturating_sub(1);
            if declared != declared_arity {
                ineligible_reason.get_or_insert_with(|| {
                    format!(
                        "'{}' must take {} argument(s), but takes {}",
                        method_name, declared_arity, declared
                    )
                });
                continue;
            }
            return HookResolution::Eligible(info);
        }
        match ineligible_reason {
            Some(reason) => HookResolution::Ineligible(reason),
            None => HookResolution::Absent,
        }
    }

    /// Resolves a hook (see [`Analyzer::resolve_hook_method`]) and, when it is unusable, emits the
    /// site-specific diagnostic for you and returns `None`: marks HIR failed (also clearing the
    /// pending value when `clear_value`), then reports `ineligible(reason)` or `absent()` at `span`.
    /// Centralizes the identical Ineligible/Absent arms every desugaring site (`obj[i]`, `obj[i] = v`,
    /// `for..in`) previously spelled out; callers keep only their `Eligible` logic.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolve_hook_or_diagnose(
        &mut self,
        obj_type: &Type,
        method_name: &str,
        declared_arity: usize,
        span: Option<crate::text::text_span::TextSpan>,
        clear_value: bool,
        diagnostics: &mut DiagnosticBag,
        ineligible: impl FnOnce(&str) -> String,
        absent: impl FnOnce() -> String,
    ) -> Option<FunctionTableInfo> {
        let message =
            match self.resolve_hook_method(obj_type, method_name, declared_arity, diagnostics) {
                HookResolution::Eligible(info) => return Some(info),
                HookResolution::Ineligible(reason) => ineligible(&reason),
                HookResolution::Absent => absent(),
            };
        self.hir_fail();
        if clear_value {
            self.hir_none();
        }
        diagnostics.report_error(message, span);
        None
    }

    /// Analyzes a static-method call `Type.method(args)` (resolved by the caller to the type
    /// `type_name`). Static methods have no implicit `this`, so the explicit arguments map 1:1 to
    /// the declared parameters.
    pub(crate) fn analyze_static_call(
        &mut self,
        type_name: &str,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let base = method_fn(type_name, &method.text);

        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        for param in params.iter() {
            let t = self.analyze_expression(param, parent_function, symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
            arg_types.push(t.get_type());
        }

        let store_sig = if self.function_table.is_overloaded(&base) {
            match self.select_function_overload(&base, &arg_types) {
                Ok(sig) => sig,
                Err(message) => {
                    return Err(report(diagnostics, message, Some(method.position)));
                }
            }
        } else {
            match self.function_table.get_function(&base) {
                Ok(s) => s.clone(),
                Err(_) => {
                    return Err(report(
                        diagnostics,
                        format!(
                            "Type '{}' has no static method '{}'",
                            type_name, method.text
                        ),
                        Some(method.position),
                    ));
                }
            }
        };

        if !store_sig.is_public && !self.in_methods_of(parent_function, type_name) {
            diagnostics.report_error(
                format!("'{}' is private to '{}'", method.text, type_name),
                Some(method.position),
            );
        }

        let expected_params = store_sig.parameters.clone();
        if expected_params.len() != arg_types.len() {
            diagnostics.report_error(
                format!(
                    "static method {} expects {} parameters, got {}",
                    base,
                    expected_params.len(),
                    arg_types.len()
                ),
                Some(method.position),
            );
            self.hir_none();
            return Ok(Type::Unknown);
        }
        for (i, given_type) in arg_types.iter().enumerate() {
            let expected = &expected_params[i];
            if expected == "object" || is_unknown_type_name(given_type) {
                continue;
            }
            if is_numeric_primitive(expected) && is_numeric_primitive(given_type) {
                continue;
            }
            if given_type != expected {
                diagnostics.report_error(
                    format!(
                        "static method {} expects parameter {} to be {}, got {}",
                        base,
                        i + 1,
                        expected,
                        given_type
                    ),
                    Some(method.position),
                );
            }
        }

        // An async static method (e.g. `File.read`) eagerly starts a task; the call yields a
        // `Future<T>` that must be `await`ed, just like any other async call.
        // An `async` static method yields a `Future<T>` handle (carried by the `Call`); `await`
        // unwraps it.
        let ret_type = if store_sig.is_async {
            Self::future_type(store_sig.return_type.unwrap_or(Type::Void))
        } else {
            store_sig.return_type.unwrap_or(Type::Void)
        };
        // A static method is implemented as an unbound function under its mangled `{Type}_{method}` name (no receiver);
        // overloaded names are ambiguous for a single `DefId` lookup, so defer those.
        if self.function_table.is_overloaded(&base) {
            self.hir_none();
        } else {
            self.hir_set_call(&base, arg_hirs, &ret_type);
        }
        Ok(ret_type)
    }
}
