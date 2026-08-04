//! The main free-function call path: dispatches indirect (function-value) calls, constructor calls,
//! generic monomorphization, and overload/arity resolution, then emits the resolved direct call.
//! Also hosts `substitute_default_args`, shared with the constructor and instance-call paths.

use super::*;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::ExpressionNode;
use crate::types::constructor_fn;

impl<'a> Analyzer<'a> {
    pub(crate) fn analyze_function_call(
        &mut self,
        name: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        // An unqualified call resolves within the caller's own declaring module first: if a
        // cross-module name collision ever promoted this base name to module-qualified keys (see
        // `FunctionTable::add_overload`), a bare reference from inside that same module must still
        // resolve to its own module's declaration rather than ambiguously landing on whichever
        // other module's declaration happens to still hold the bare key (or erroring outright).
        // A no-op (returns the name unchanged) for the overwhelming majority of names, which were
        // never involved in such a collision.
        let caller_module = self.module_of(parent_function.file_path.as_ref());
        let mut function_name = self
            .function_table
            .resolve_in_module(caller_module.as_ref(), &name.text)
            .map(|s| s.to_string())
            .unwrap_or_else(|| name.text.clone());
        let mut params_types = vec![];
        let mut arg_hirs = vec![];
        // A named argument (`f(x, y: 2)`) must be reordered to its positional slot, and a variadic
        // call's trailing loose arguments collected into an array, *before* any argument is
        // analyzed below (analysis is index-driven). Both are only possible when the callee
        // resolves to exactly one known parameter-name list up front: a non-overloaded free
        // function, or a plain (non-generic-base) constructor call. Overloaded/indirect/
        // unresolvable callees report a clear diagnostic instead of guessing.
        let has_named_arg = params
            .iter()
            .any(|a| matches!(a, ExpressionNode::NamedArg(..)));
        let callee_signature = |analyzer: &Self| -> Option<(Vec<String>, Vec<Option<Type>>, bool)> {
            if let Ok(info) = analyzer.function_table.get_function(&function_name) {
                Some((info.param_names, info.defaults, info.is_variadic))
            } else if generic_args.is_none()
                && analyzer.struct_table.get_struct(&function_name).is_some()
            {
                analyzer
                    .function_table
                    .get_function(&constructor_fn(&function_name))
                    .ok()
                    .map(|info| {
                        (
                            info.param_names.iter().skip(1).cloned().collect(),
                            info.defaults.iter().skip(1).cloned().collect(),
                            info.is_variadic,
                        )
                    })
            } else {
                None
            }
        };
        let normalized_params: Vec<ExpressionNode<'a>>;
        let params: &[ExpressionNode<'a>] =
            if has_named_arg || { callee_signature(self).is_some_and(|(_, _, variadic)| variadic) }
            {
                if self.function_table.is_overloaded(&function_name) {
                    return Err(report(
                        diagnostics,
                        format!(
                        "named/variadic arguments are not supported for overloaded function '{}'",
                        function_name
                    ),
                        Some(name.position),
                    ));
                }
                let Some((param_names, defaults, is_variadic)) = callee_signature(self) else {
                    return Err(report(
                        diagnostics,
                        format!("named arguments are not supported for '{}'", function_name),
                        Some(name.position),
                    ));
                };
                if has_named_arg && is_variadic {
                    return Err(report(
                        diagnostics,
                        format!(
                            "named arguments are not supported for variadic function '{}'",
                            function_name
                        ),
                        Some(name.position),
                    ));
                }
                normalized_params = if has_named_arg {
                    self.normalize_named_arguments(
                        &param_names,
                        &defaults,
                        params,
                        name.position,
                        diagnostics,
                    )?
                } else {
                    self.collect_variadic_args(param_names.len(), params)
                };
                &normalized_params
            } else {
                params.as_slice()
            };
        // When the callee is an unambiguous (non-overloaded) free function, publish each parameter's
        // declared type as the expected type while analyzing the matching argument, so untyped
        // literals such as an empty array `[]` infer their element type from the signature. A plain
        // (non-generic) constructor call gets the same treatment via its `constructor`'s parameter
        // types, so e.g. an `Option<T>`-typed field's `None`/`Some(...)` argument can infer `T`
        // without an explicit annotation.
        let expected_params: Option<Vec<Type>> = if self
            .function_table
            .is_overloaded(&function_name)
        {
            None
        } else if let Ok(info) = self.function_table.get_function(&function_name) {
            Some(Self::expected_param_types(&info))
        } else if generic_args.is_none() && self.struct_table.get_struct(&function_name).is_some() {
            self.function_table
                .get_function(&constructor_fn(&function_name))
                .ok()
                .map(|info| {
                    Self::expected_param_types(&info)
                        .into_iter()
                        .skip(1)
                        .collect()
                })
        } else {
            None
        };
        let mut arg_is_ref: Vec<bool> = Vec::with_capacity(params.len());
        for (i, param) in params.iter().enumerate() {
            let saved_expected = self.current_expected_type.take();
            self.current_expected_type = expected_params.as_ref().and_then(|ps| ps.get(i).cloned());
            if let ExpressionNode::RefArgument(inner) = param {
                arg_is_ref.push(true);
                self.current_expected_type = saved_expected;
                match self.analyze_ref_argument(inner, symbol_table, diagnostics) {
                    Some((t, hir)) => {
                        arg_hirs.push(hir);
                        params_types.push(t.get_type());
                    }
                    None => {
                        arg_hirs.push(None);
                        params_types.push(Type::Unknown.get_type());
                    }
                }
                continue;
            }
            arg_is_ref.push(false);
            let t = self.analyze_expression(param, parent_function, symbol_table, diagnostics)?;
            self.current_expected_type = saved_expected;
            arg_hirs.push(self.hir_take());
            params_types.push(t.get_type());
        }
        // Calling a `js`-typed local (`cb(a, b)`) invokes the underlying JS value dynamically.
        let name_sym = (*symbol_table).as_ref().borrow().get_symbol(name);
        if let Ok(sym_ty) = name_sym {
            if self.is_js_type(&sym_ty) {
                self.hir_set_var(&name.text);
                let recv = self.hir_take();
                self.desugar_js_invoke(recv, arg_hirs, Some(name.position), diagnostics);
                return Ok(Self::js_type());
            }
        }

        // Default: no call HIR. Only the plain free-function tail below opts back in; every other
        // path (indirect, constructor, generic, async, overload/arity errors) leaves `last` cleared.
        self.hir_none();

        // Indirect call: if the called name is a local variable of function type, validate the
        // arguments against the function-type signature and return its result type.
        if let Ok(Type::Function(param_types, ret)) =
            (*symbol_table).as_ref().borrow().get_symbol(name)
        {
            if param_types.len() != params_types.len() {
                diagnostics.report_error(
                    format!(
                        "function value '{}' expects {} arguments, got {}",
                        name.text,
                        param_types.len(),
                        params_types.len()
                    ),
                    Some(name.position),
                );
                return Ok((*ret).clone());
            }
            let expected_strs: Vec<String> = param_types.iter().map(|t| t.get_type()).collect();
            self.validate_arguments(
                &format!("function value '{}'", name.text),
                &expected_strs,
                &params_types,
                name.position,
                diagnostics,
            );
            self.hir_set_indirect_call(&name.text, arg_hirs, ret.as_ref());
            return Ok((*ret).clone());
        }

        // Interfaces cannot be instantiated: `Animal()` is an error even though `Animal` names a
        // type, because an interface has no fields/constructor and no concrete runtime layout.
        if self.type_ctx.nominal_kind(&function_name) == Some(crate::types::DefKind::Interface) {
            return Err(report(
                diagnostics,
                format!("cannot instantiate interface '{}'", function_name),
                Some(name.position),
            ));
        }

        // Constructor call: `Struct(args)` / `Struct<T>(args)`. Only treated as a constructor
        // when no free function (concrete or generic) shadows the name, so prelude factory
        // functions such as `List<T>()` keep their behaviour.
        if self.function_table.get_function(&function_name).is_err()
            && !self.function_table.is_overloaded(&function_name)
            && !self.generic_functions.contains_key(&function_name)
            && (self.struct_table.get_struct(&function_name).is_some()
                || self.generic_structs.contains_key(&function_name))
        {
            // Substitute the enclosing monomorphization's bindings into the type arguments, so a
            // generic construction using a type parameter (`ListIterator<T>(this)` inside a
            // monomorphized `List<string>.iterator`) instantiates the concrete `ListIterator_string`
            // rather than the unsubstituted `ListIterator_T`.
            let concrete_generic_args: Option<Vec<Type>> = generic_args.as_ref().map(|g| {
                g.iter()
                    .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings))
                    .collect()
            });
            let t = self.analyze_constructor_call(
                name,
                &concrete_generic_args,
                &mut params_types,
                &mut arg_hirs,
                parent_function,
                symbol_table,
                diagnostics,
            )?;
            // The concrete struct whose layout the backend uses: a plain struct is its own name, a
            // generic instance (`Box<int>`) its mangled name (`Box_int`), which
            // `ensure_struct_instantiated` has already added to the struct table. A generic base with
            // no type args is an error, not a constructor. When the instance is registered, emit
            // `New`: if it declares a user `constructor(){}`, resolve that def so the backend calls it
            // (its args are the constructor's); otherwise the implicit zero-arg default constructor
            // takes no args and leaves every field at its zero value.
            // `hir_set_new` is given the source (base) name — the registered `DefId` for both plain
            // and generic structs — while the result type `t` supplies the per-instance layout key.
            let concrete_name = match &concrete_generic_args {
                Some(g) if !g.is_empty() => Some(mangle_generic(&name.text, g)),
                _ if !self.generic_structs.contains_key(&name.text) => Some(name.text.clone()),
                _ => None,
            };
            if let Some(concrete_name) = concrete_name {
                if self.struct_table.get_struct(&concrete_name).is_some() {
                    let ctor = self.type_ctx.defs.lookup(
                        crate::types::DefKind::Function,
                        &constructor_fn(&concrete_name),
                    );
                    self.hir_set_new(&name.text, ctor, arg_hirs, &t);
                }
            }
            return Ok(t);
        }

        // (generic function instantiation is factored into `register_generic_function_instance`.)

        // The base (template) name + instance type-arg names for a generic call, captured so HIR
        // emission can resolve the call to the shared base `DefId` plus the monomorphization args.
        // The names are lowered with the same `lower_str` the instance body uses, so the symbols
        // agree.
        let mut generic_instance: Option<(String, Vec<Type>)> = None;

        // Monomorphization: bind every generic parameter to a concrete type, then register
        // (once) a specialized signature under the mangled name.
        if self.generic_functions.contains_key(&function_name) {
            let template = match self.generic_functions.get(&function_name) {
                Some(template) => *template,
                None => {
                    diagnostics.report_error(
                        format!("Generic function '{}' could not be resolved", function_name),
                        Some(name.position),
                    );
                    return Ok(Type::Unknown);
                }
            };
            let bindings = self.infer_generic_bindings(
                template,
                generic_args,
                &params_types,
                &name.position,
                diagnostics,
            );
            // A constrained type parameter (`fun sort<T : Comparable<T>>(...)`) must be satisfied by
            // the concrete argument; report a clear error at the call site otherwise.
            self.verify_generic_constraints(
                &template.generic_constraints,
                &bindings,
                &name.position,
                diagnostics,
            );
            let mangled_name = self.register_generic_function_instance(template, &bindings);
            generic_instance = Some((function_name.clone(), bindings.values().cloned().collect()));
            function_name = mangled_name;
        }

        // Overloaded free functions resolve by argument types; non-overloaded names keep the
        // direct single-signature lookup (and its precise per-argument diagnostics below).
        let store_sig = if self.function_table.is_overloaded(&function_name) {
            match self.select_function_overload(&function_name, &params_types) {
                Ok(sig) => sig,
                Err(message) => {
                    return Err(report(diagnostics, message, Some(name.position)));
                }
            }
        } else {
            match self.function_table.get_function(&function_name) {
                Ok(sig) => sig,
                Err(e) => {
                    return Err(report(diagnostics, e.to_string(), Some(name.position)));
                }
            }
        };

        // File/module-level visibility (Axis 2): a non-public free function is only callable from
        // its own file. Static methods dispatched here (mangled `Type_method`) keep their own
        // class-level check in `analyze_static_call`.
        if !self.visible_across_files(
            &store_sig.declaring_file,
            store_sig.visibility,
            parent_function.file_path.as_ref(),
        ) {
            self.report_not_public(
                "Function",
                &name.text,
                &store_sig.declaring_file,
                name.position,
                diagnostics,
            );
        }

        self.check_unsafe_call(&store_sig, name.position, diagnostics);

        self.validate_ref_arguments(
            &format!("function '{}'", function_name),
            &store_sig.is_ref,
            &arg_is_ref,
            name.position,
            diagnostics,
        );

        let required = store_sig.required_params();
        let total = store_sig.parameters.len();
        let given = params_types.len();
        if given < required || given > total {
            let message = if required == total {
                format!(
                    "Function {} has {} params but {} params are given",
                    function_name, total, given
                )
            } else {
                format!(
                    "Function {} expects between {} and {} arguments, got {}",
                    function_name, required, total, given
                )
            };
            diagnostics.report_error(message, Some(name.position));
            return Ok(Type::Unknown);
        }

        // Substitute default values for any omitted trailing parameters. Each default is a constant
        // literal, so analyzing `Literal(default)` produces the same type-string and HIR an explicit
        // literal argument would, and feeds the per-index checks and `hir_set_call` below unchanged.
        self.substitute_default_args(
            &store_sig.defaults,
            &mut params_types,
            &mut arg_hirs,
            parent_function,
            symbol_table,
            diagnostics,
        )?;

        self.validate_arguments(
            &format!("function '{}'", function_name),
            &store_sig.parameters,
            &params_types,
            name.position,
            diagnostics,
        );

        let ret_type = Self::async_return_type(store_sig.is_async, store_sig.return_type);
        // Emit a resolved direct call. A generic call resolves to the template's base `DefId` plus
        // the monomorphization args (so it targets the emitted instance); a plain non-overloaded
        // free function resolves by name. Overloads would collide on the base name's single `DefId`,
        // so they stay on the legacy path for now.
        if let Some((base_name, instance_types)) = generic_instance {
            let instance = instance_types
                .iter()
                .map(|t| self.type_ctx.lower(t))
                .collect();
            self.hir_set_generic_call(&base_name, instance, arg_hirs, &ret_type);
        } else {
            // Overloaded free functions resolve to the selected overload's emitted name (each is a
            // distinct `DefId`); non-overloaded ones resolve directly by their base name.
            self.hir_set_call(&store_sig.name, arg_hirs, &ret_type);
        }
        Ok(ret_type)
    }

    /// Appends the default values of any omitted trailing parameters to a call's argument lists.
    /// `defaults` is the callee's per-parameter default slice (parallel to its parameters); for each
    /// index at or past the number of supplied arguments that carries a default, its constant
    /// literal is analyzed exactly like an explicit literal argument, extending both `params_types`
    /// (for the per-index type check) and `arg_hirs` (for the emitted call). Callers must have
    /// already validated arity (supplied count within `required..=total`).
    pub(crate) fn substitute_default_args(
        &mut self,
        defaults: &[Option<Type>],
        params_types: &mut Vec<String>,
        arg_hirs: &mut Vec<Option<crate::hir::HExpr>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        for i in params_types.len()..defaults.len() {
            if let Some(default) = defaults.get(i).and_then(|d| d.clone()) {
                let lit = ExpressionNode::Literal(default);
                let t =
                    self.analyze_expression(&lit, parent_function, symbol_table, diagnostics)?;
                arg_hirs.push(self.hir_take());
                params_types.push(t.get_type());
            }
        }
        Ok(())
    }
}
