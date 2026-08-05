//! Ordinary and interface instance-method resolution once the receiver type is known and the
//! static/builtin cases have been ruled out.

use super::super::super::*;
use dream_diagnostics::DiagnosticBag;
use crate::errors::SemanticError;
use dream_syntax::nodes::types::mangle_generic;
use dream_syntax::nodes::{ExpressionNode, FunctionNode, Type};
use dream_syntax::token::syntax_token::SyntaxToken;
use dream_types::method_fn;

impl<'a> Analyzer<'a> {
    /// If `obj_type` names an interface, returns that interface's name; otherwise `None`.
    pub(crate) fn interface_receiver_name(&self, obj_type: &Type) -> Option<String> {
        let name = obj_type.get_type();
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
        ctx: &super::super::super::AnalyzerContext<'a, '_>,
        receiver: Option<dream_hir::HExpr>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let (arg_types, arg_hirs) = self.analyze_call_arguments(
            params,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;

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
        let ret_type = Self::async_return_type(im.is_async, im.return_type.clone());
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
    ) -> dream_types::TypeId {
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

    /// Resolves and type-checks an instance method call `obj.method(args)` once the receiver type
    /// (`obj_type`) is known and the builtins/static cases have been ruled out: monomorphizes the
    /// Resolves an ordinary instance method call on a concrete (non-interface) receiver. Instantiates
    /// a generic struct receiver, selects the (possibly overloaded) `{Type}_{method}`, enforces privacy and the
    /// argument arity/types, and returns the call's result type (a `Future<T>` for `async`).
    /// Method-level generics (`obj.method<T>(...)`) are monomorphized on the fly, mirroring static
    /// generic methods.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn analyze_instance_method(
        &mut self,
        obj_type: &Type,
        method: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::super::AnalyzerContext<'a, '_>,
        receiver: Option<dream_hir::HExpr>,
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
            None => obj_type.get_type(),
        };

        let mangled_name = method_fn(&struct_name, &method.text);

        // Method-level generics (`pool.dispatch<TIn, TOut>(...)`): monomorphize before the plain
        // `function_table` path, which only knows the unbound template signature.
        if let Some(&template) = self.generic_functions.get(&mangled_name) {
            return self.analyze_generic_instance_method(
                template,
                &mangled_name,
                &struct_name,
                method,
                generic_args,
                params,
                ctx,
                receiver,
                diagnostics,
            );
        }

        // Reorder named arguments (`obj.method(x, y: 2)`) to positional and collect a
        // non-overloaded variadic call's trailing arguments into an array before index-driven
        // analysis. Overloaded positional variadic calls stay unpacked until after selection.
        let has_named_arg = params
            .iter()
            .any(|a| matches!(a, ExpressionNode::NamedArg(..)));
        let method_info = self.function_table.get_function(&mangled_name).ok();
        let is_variadic = method_info.as_ref().is_some_and(|info| info.is_variadic);
        let is_overloaded = self.function_table.is_overloaded(&mangled_name);
        let normalized_params: Vec<ExpressionNode<'a>>;
        let params: &[ExpressionNode<'a>] = if has_named_arg {
            if is_overloaded {
                normalized_params = self.normalize_named_for_overloads(
                    &mangled_name,
                    params,
                    method.position,
                    1,
                    diagnostics,
                )?;
            } else {
                let Some(info) = method_info else {
                    return Err(report(
                        diagnostics,
                        format!("Type '{}' has no method '{}'", struct_name, method.text),
                        Some(method.position),
                    ));
                };
                let param_names: Vec<String> = info.param_names.iter().skip(1).cloned().collect();
                let defaults: Vec<Option<Type>> = info.defaults.iter().skip(1).cloned().collect();
                normalized_params = self.normalize_named_arguments(
                    &param_names,
                    &defaults,
                    params,
                    method.position,
                    diagnostics,
                    info.is_variadic,
                )?;
            }
            &normalized_params
        } else if is_variadic && !is_overloaded {
            let info = method_info.unwrap();
            let param_names_len = info.param_names.len().saturating_sub(1);
            normalized_params = self.collect_variadic_args(param_names_len, params);
            &normalized_params
        } else {
            params.as_slice()
        };

        // When the method is unambiguous (not overloaded), its declared parameter types are known
        // before the arguments are analyzed, so publish them as each argument's expected type (same
        // treatment as an unambiguous free-function call) — this lets an argument lambda without its
        // own type context (e.g. `nums.sort_by((a: int, b: int) => a - b)`) infer from the `fun(...)`
        // parameter type. An overloaded method's parameter types aren't known until the arguments
        // themselves are typed, so it falls back to no expected-type context (unchanged behavior).
        let expected_params: Option<Vec<Type>> = if self.function_table.is_overloaded(&mangled_name)
        {
            self.expected_params_preferring_fun_overload(&mangled_name, params, 1)
        } else {
            self.function_table
                .get_function(&mangled_name)
                .ok()
                .map(|info| {
                    info.parameters
                        .iter()
                        .skip(1) // implicit `this`
                        .map(|p| Self::type_from_name(p))
                        .collect()
                })
        };

        let call_target = format!("{}.{}", struct_name, method.text);
        let saved_call_target = self.current_call_target_name.take();
        self.current_call_target_name = Some(call_target);

        // Analyze the explicit arguments once, then resolve the method (overloaded methods select
        // by argument types, with the receiver supplied as the implicit `this` argument).
        let (mut arg_types, mut arg_hirs, mut arg_is_ref) = self.analyze_call_arguments_expecting_ref(
            params,
            expected_params.as_deref(),
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;

        self.current_call_target_name = saved_call_target;

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

        self.pack_variadic_analyzed_args(
            &store_sig,
            &mut arg_types,
            &mut arg_hirs,
            &mut arg_is_ref,
            1,
        );

        // Private methods (the default) may only be called from within the declaring type's own
        // methods; `internal` from anywhere in the same module; `public` exposes them everywhere.
        if !store_sig.visibility.is_public() {
            let base_name = Self::resolve_struct_parts(obj_type)
                .map(|(b, _)| b)
                .unwrap_or_else(|| obj_type.get_type());
            if !self.member_accessible(
                store_sig.visibility,
                &store_sig.declaring_file,
                ctx.parent_function.file_path.as_ref(),
                self.in_methods_of(ctx.parent_function, &base_name),
            ) {
                diagnostics.report_error(
                    format!("'{}' is private to '{}'", method.text, base_name),
                    Some(method.position),
                );
            }
        }

        self.check_unsafe_call(&store_sig, method.position, diagnostics);

        let mut expected_params = store_sig.parameters.clone();
        let mut expected_defaults = store_sig.defaults.clone();
        let mut expected_is_ref = store_sig.is_ref.clone();

        // Remove 'this' from the expected params check since we supply it implicitly
        if !expected_params.is_empty() {
            expected_params.remove(0);
        }
        if !expected_defaults.is_empty() {
            expected_defaults.remove(0);
        }
        if !expected_is_ref.is_empty() {
            expected_is_ref.remove(0);
        }
        self.validate_ref_arguments(
            &format!("method '{}'", method.text),
            &expected_is_ref,
            &arg_is_ref,
            method.position,
            diagnostics,
        );

        let total = expected_params.len();
        let required = Self::required_arg_count(&expected_defaults, total);
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

        // An `async` method yields a `Future<T>` handle (carried by the `MethodCall`); `await`
        // unwraps it.
        let ret_type = Self::async_return_type(store_sig.is_async, store_sig.return_type);
        // Overloaded methods each register a distinct `DefId` under their emitted (signature-mangled)
        // name; resolve to the selected overload's name so the call targets the right instance.
        // Non-overloaded methods keep their base-mangled name.
        self.hir_set_method_call(receiver, &store_sig.name, arg_hirs, &ret_type);
        Ok(ret_type)
    }

    /// Monomorphizes a method-level generic instance call (`obj.method<T>(args)`). Mirrors
    /// [`analyze_generic_static_method`]: infer/bind type args, register a concrete instance, emit
    /// a `MethodCall` whose `Callee.instance` carries the TypeIds so WASM symbols match the body.
    #[allow(clippy::too_many_arguments)]
    fn analyze_generic_instance_method(
        &mut self,
        template: &'a FunctionNode<'a>,
        base: &str,
        struct_name: &str,
        method: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::super::AnalyzerContext<'a, '_>,
        receiver: Option<dream_hir::HExpr>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        // Explicit type args let us publish monomorphized parameter types as expected types for
        // argument lambdas (`pool.dispatch<int,int>(5, (n) => n + 1)`).
        let early_bindings = if generic_args.as_ref().is_some_and(|g| !g.is_empty()) {
            Some(self.infer_generic_bindings(
                template,
                generic_args,
                &[],
                &method.position,
                diagnostics,
            ))
        } else {
            None
        };

        let expected_params: Option<Vec<Type>> = early_bindings.as_ref().map(|bindings| {
            template
                .parameters
                .iter()
                .skip(1) // implicit `this`
                .map(|p| Self::monomorphize_type(&p.type_, bindings))
                .collect()
        });

        let call_target = format!("{}.{}", struct_name, method.text);
        let saved_call_target = self.current_call_target_name.take();
        self.current_call_target_name = Some(call_target);

        let (mut arg_types, mut arg_hirs, mut arg_is_ref) = self.analyze_call_arguments_expecting_ref(
            params,
            expected_params.as_deref(),
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;

        self.current_call_target_name = saved_call_target;

        // Align with the template's parameter list (index 0 is `this`) for inference.
        let mut inference_types: Vec<String> = Vec::with_capacity(arg_types.len() + 1);
        inference_types.push(struct_name.to_string());
        inference_types.extend(arg_types.iter().cloned());

        let bindings = early_bindings.unwrap_or_else(|| {
            self.infer_generic_bindings(
                template,
                generic_args,
                &inference_types,
                &method.position,
                diagnostics,
            )
        });

        if !self.member_accessible(
            template.visibility,
            &template.file_path,
            ctx.parent_function.file_path.as_ref(),
            self.in_methods_of(ctx.parent_function, struct_name),
        ) {
            diagnostics.report_error(
                format!("'{}' is private to '{}'", method.text, struct_name),
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

        self.check_unsafe_call(&store_sig, method.position, diagnostics);

        let mut expected_params = store_sig.parameters.clone();
        let mut expected_defaults = store_sig.defaults.clone();
        let mut expected_is_ref = store_sig.is_ref.clone();
        if !expected_params.is_empty() {
            expected_params.remove(0);
        }
        if !expected_defaults.is_empty() {
            expected_defaults.remove(0);
        }
        if !expected_is_ref.is_empty() {
            expected_is_ref.remove(0);
        }

        self.pack_variadic_analyzed_args(
            &store_sig,
            &mut arg_types,
            &mut arg_hirs,
            &mut arg_is_ref,
            1,
        );

        self.validate_ref_arguments(
            &format!("method '{}'", method.text),
            &expected_is_ref,
            &arg_is_ref,
            method.position,
            diagnostics,
        );

        let total = expected_params.len();
        let required = Self::required_arg_count(&expected_defaults, total);
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

        let ret_type = Self::async_return_type(store_sig.is_async, store_sig.return_type.clone());
        let instance = bindings
            .values()
            .map(|t| self.type_ctx.lower(t))
            .collect();
        // `base` is the template's `{Type}_{method}` DefId shared by every monomorphization.
        self.hir_set_generic_method_call(receiver, base, instance, arg_hirs, &ret_type);
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
            .unwrap_or_else(|| first.type_.get_type());
        this_base == base_name
            || this_base.starts_with(&format!("{}_", base_name))
            || base_name.starts_with(&format!("{}_", this_base))
    }
}
