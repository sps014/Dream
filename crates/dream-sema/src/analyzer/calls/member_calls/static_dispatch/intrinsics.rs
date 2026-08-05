//! On-the-fly monomorphization of a generic static method, dispatching the `System.print`,
//! `Buffer.alloc`, `Bytes.of`/`to`, `Promise.*`, and `JSON.serialize`/`deserialize` intrinsics
//! before falling back to registering a plain generic-static instance.

use super::*;
use dream_abi::intrinsics;
use dream_syntax::nodes::types::is_unknown_type_name;

/// Call-site bundle for [`Analyzer::analyze_generic_static_method`]: the parsed pieces of a
/// `Type.method(args)` call already resolved to a generic static method template, kept together
/// so the analysis function itself only needs the bundle plus the analyzer context/diagnostics.
pub(super) struct GenericStaticMethodCall<'a, 'b> {
    pub(super) template: &'a FunctionNode<'a>,
    pub(super) base: &'b str,
    pub(super) type_name: &'b str,
    pub(super) method: &'b SyntaxToken,
    pub(super) generic_args: &'b Option<Vec<Type>>,
    pub(super) params: &'b Vec<ExpressionNode<'a>>,
}

impl<'a> Analyzer<'a> {
    /// Resolves a `Type.method(args)` call whose `{Type}_{method}` names a generic static method
    /// (`call.template`). Handles the recognized intrinsics inline and otherwise registers a
    /// monomorphized instance. Always resolves to a type (the outer dispatch wraps it in `Some`);
    /// `call.base` is the mangled `{Type}_{method}` symbol and `call.type_name` the receiver
    /// type's name.
    pub(super) fn analyze_generic_static_method(
        &mut self,
        call: GenericStaticMethodCall<'a, '_>,
        ctx: &AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let GenericStaticMethodCall {
            template,
            base,
            type_name,
            method,
            generic_args,
            params,
        } = call;
        let mut params_types = vec![];
        let mut arg_hirs = vec![];
        for param in params.iter() {
            let t =
                self.analyze_expression(param, ctx.parent_function, ctx.symbol_table, diagnostics)?;
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
            return Ok(Type::Unknown);
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
            return Ok(Type::Array(Box::new(element)));
        }

        // `Buffer.realloc<T>(arr, new_len)` (`@unsafe`): in-place `$realloc`-based grow/shrink of
        // `arr`'s backing block, returning a `T[]` of `new_len` elements.
        if intrinsics::IntrinsicOp::from_attributes(&template.attributes)
            == Some(intrinsics::IntrinsicOp::ArrayRealloc)
        {
            self.check_unsafe_intrinsic_call(
                "Buffer.realloc",
                template,
                method.position,
                diagnostics,
            );
            let element = match generic_args.as_ref().and_then(|g| g.first()) {
                Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                None => params_types
                    .first()
                    .map(|s| s.trim_end_matches("[]").to_string())
                    .map(|s| {
                        let mut t = method.clone();
                        t.text = s;
                        Type::from_token(t).unwrap_or(Type::Unknown)
                    })
                    .unwrap_or(Type::Unknown),
            };
            if params_types.len() != 2 {
                diagnostics.report_error(
                    format!(
                        "'Buffer.realloc' expects exactly 2 arguments (array, new length), got {}",
                        params_types.len()
                    ),
                    Some(method.position),
                );
            }
            let mut args = arg_hirs.into_iter();
            let array = args.next().flatten();
            let new_len = args.next().flatten();
            self.hir_set_array_realloc(&element, array, new_len);
            return Ok(Type::Array(Box::new(element)));
        }

        // `Buffer.free<T>(arr)` (`@unsafe`): unconditionally returns `arr`'s backing block to the
        // allocator, bypassing reference counting.
        if intrinsics::IntrinsicOp::from_attributes(&template.attributes)
            == Some(intrinsics::IntrinsicOp::ForceFree)
        {
            self.check_unsafe_intrinsic_call("Buffer.free", template, method.position, diagnostics);
            if params_types.len() != 1 {
                diagnostics.report_error(
                    format!(
                        "'Buffer.free' expects exactly 1 argument (the array), got {}",
                        params_types.len()
                    ),
                    Some(method.position),
                );
            }
            self.hir_set_force_free(arg_hirs.into_iter().next().flatten());
            return Ok(Type::Void);
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
                None => params_types
                    .first()
                    .map(|s| named(s))
                    .unwrap_or(Type::Unknown),
            };
            self.require_unmanaged_or_array(&payload, "Bytes.of", &method.position, diagnostics);
            self.hir_set_to_bytes(arg_hirs.into_iter().next().flatten());
            return Ok(Type::Array(Box::new(named("byte"))));
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
            self.require_unmanaged_or_array(&target, "Bytes.to", &method.position, diagnostics);
            self.hir_set_from_bytes(&target, arg_hirs.into_iter().next().flatten());
            return Ok(target);
        }

        // `Bytes.toWire<T>(v)` / `Bytes.fromWire<T>(s)`: the `WebWorker` wire marshal. `T = string`
        // is an identity passthrough (the wire already is a `string`); any other `T` must be
        // `unmanaged` and goes through a raw byte-blit (`Bytes.of`/`to`) re-encoded as a
        // codepoint-per-byte `string` (`Bytes.toWireString`/`fromWireString`).
        if byte_op == Some(intrinsics::IntrinsicOp::WireEncode) {
            let named = |name: &str| -> Type {
                let mut t = method.clone();
                t.text = name.to_string();
                Type::from_token(t).unwrap_or(Type::Unknown)
            };
            let payload = match generic_args.as_ref().and_then(|g| g.first()) {
                Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                None => params_types
                    .first()
                    .map(|s| named(s))
                    .unwrap_or(Type::Unknown),
            };
            let value = arg_hirs.into_iter().next().flatten();
            // A still-abstract type parameter means this call sits inside a generic struct's own
            // declaration-time analysis pass (its methods are fully HIR-emitted once using the
            // class's type parameters as literal placeholder types, in addition to once per real
            // instantiation - unlike generic free functions, there is no "skip the unbound pass"
            // path for struct-level generics). That placeholder body is never actually reached by
            // any real call site, so its HIR just needs to type-check structurally: treat `T` as
            // `string` (identity passthrough) rather than trying to validate an unresolvable bound.
            if self.is_unresolved_generic_type(&payload) || payload.get_type() == "string" {
                self.hir_set_last(value);
            } else {
                self.require_unmanaged_or_array(&payload, "Bytes.toWire", &method.position, diagnostics);
                self.hir_set_to_bytes(value);
                let bytes = self.hir_take();
                self.hir_set_call("Bytes_toWireString", vec![bytes], &named("string"));
            }
            return Ok(named("string"));
        }
        if byte_op == Some(intrinsics::IntrinsicOp::WireDecode) {
            let target = match generic_args.as_ref().and_then(|g| g.first()) {
                Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                None => {
                    diagnostics.report_error(
                        "'Bytes.fromWire' requires a type argument, e.g. Bytes.fromWire<T>(text)"
                            .to_string(),
                        Some(method.position),
                    );
                    Type::Void
                }
            };
            let text = arg_hirs.into_iter().next().flatten();
            // See the matching comment in the `WireEncode` arm above: a dead placeholder body from
            // a generic struct's declaration-time analysis pass, never reached by a real call site.
            if self.is_unresolved_generic_type(&target) || target.get_type() == "string" {
                self.hir_set_last(text);
            } else {
                self.require_unmanaged_or_array(&target, "Bytes.fromWire", &method.position, diagnostics);
                let named = |name: &str| -> Type {
                    let mut t = method.clone();
                    t.text = name.to_string();
                    Type::from_token(t).unwrap_or(Type::Unknown)
                };
                self.hir_set_call(
                    "Bytes_fromWireString",
                    vec![text],
                    &Type::Array(Box::new(named("byte"))),
                );
                let bytes = self.hir_take();
                self.hir_set_from_bytes(&target, bytes);
            }
            return Ok(target);
        }

        let bindings = self.infer_generic_bindings(
            template,
            generic_args,
            &params_types,
            &method.position,
            diagnostics,
        );

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
            self.hir_set_call(base, vec![arg_hir], &ret);
            return Ok(ret);
        }

        // `JSON.serialize<T>(v)` / `JSON.deserialize<T>(text)`: the `@json` derive emits
        // `<T>.to_json()` / `<T>.from_json()` (see `driver::json_derive`), and `JSON.stringify` /
        // `JSON.parse` are ordinary static methods. Expand the intrinsic into that composition so
        // the whole thing lowers through MIR as ordinary calls.
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
            return Ok(named("string"));
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
            return Ok(t_type);
        }

        // Class-level privacy (Axis 1): a non-public generic static method is private to its
        // declaring type, exactly like the non-generic path in `analyze_static_call`. Without
        // this the generic branch below would return early and skip the check entirely.
        if !self.member_accessible(
            template.visibility,
            &template.file_path,
            ctx.parent_function.file_path.as_ref(),
            self.in_methods_of(ctx.parent_function, type_name),
        ) {
            diagnostics.report_error(
                format!("'{}' is private to '{}'", method.text, type_name),
                Some(method.position),
            );
        }

        self.verify_generic_constraints(
            &template.generic_constraints,
            &bindings,
            &method.position,
            diagnostics,
        );
        let mangled_name = self.register_generic_function_instance(template, &bindings);

        let store_sig = match self.function_table.get_function(&mangled_name) {
            Ok(sig) => sig,
            Err(_) => {
                diagnostics.report_error(
                    format!("Function '{}' could not be instantiated", mangled_name),
                    Some(method.position),
                );
                return Ok(Type::Unknown);
            }
        };

        let required = store_sig.required_params();
        let total = store_sig.parameters.len();
        let given = params_types.len();
        if given < required || given > total {
            let message = if required == total {
                format!(
                    "Function {} has {} params but {} params are given",
                    mangled_name, total, given
                )
            } else {
                format!(
                    "Function {} expects between {} and {} arguments, got {}",
                    mangled_name, required, total, given
                )
            };
            diagnostics.report_error(message, Some(method.position));
            return Ok(Type::Unknown);
        }

        self.substitute_default_args(
            &store_sig.defaults,
            &mut params_types,
            &mut arg_hirs,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;

        self.validate_arguments(
            &format!("function '{}'", mangled_name),
            &store_sig.parameters,
            &params_types,
            method.position,
            diagnostics,
        );

        let ret_type = Self::async_return_type(store_sig.is_async, store_sig.return_type);
        let instance = bindings
            .values()
            .map(|t| self.type_ctx.lower(t))
            .collect();
        // `base` is the template's `{Type}_{method}` DefId shared by every monomorphization.
        self.hir_set_generic_call(base, instance, arg_hirs, &ret_type);
        Ok(ret_type)
    }
}
