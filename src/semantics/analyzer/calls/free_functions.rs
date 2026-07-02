//! Analysis of call expressions: free-function and overload resolution, method calls, static /
//! namespaced calls (`Math.*` / `JSON.*` / async intrinsics / `derive` helpers), and constructors.

use super::super::*;
use crate::diagnostics::DiagnosticBag;
use crate::intrinsics;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::FunctionTableInfo;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::mangle_generic;
use crate::types::constructor_fn;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use std::cell::RefCell;
use std::rc::Rc;

/// Outcome of looking up an indexer/enumerator "hook" method (`get`/`set`/`iterator`/`next`) on a
/// struct receiver, for the desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.

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
        let mut function_name = name.text.clone();
        let mut params_types = vec![];
        let mut arg_hirs = vec![];
        for param in params.iter() {
            let t = self.analyze_expression(param, parent_function, symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
            params_types.push(t.get_type());
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
                name.position.clone(),
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
                    let ctor = self
                        .type_ctx
                        .defs
                        .lookup(crate::types::DefKind::Function, &constructor_fn(&concrete_name));
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
            let template = *self.generic_functions.get(&function_name).unwrap();
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
            generic_instance = Some((
                function_name.clone(),
                bindings.values().cloned().collect(),
            ));
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
            name.position.clone(),
            diagnostics,
        );

        //let r_type=&store_sig.return_type;
        // Calling an `async fun` is eager and yields a `Future<T>` handle (where `T` is the
        // declared return type). It is NOT auto-awaited; `await` retrieves the `T`.
        // Calling an `async fun` is eager and yields a `Future<T>` handle; the `Call` carries that
        // future type and an enclosing `await` unwraps it.
        let ret_type = if store_sig.is_async {
            Self::future_type(store_sig.return_type.unwrap_or(Type::Void))
        } else {
            store_sig.return_type.unwrap_or(Type::Void)
        };
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


    /// Types the async intrinsics: `sleep(ms: int): Future<void>`, `all(xs: Future<T>[]):
    /// Future<T[]>`, `any`/`race(xs: Future<T>[]): Future<T>`.
    pub(crate) fn analyze_async_intrinsic(
        &mut self,
        name: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        if name.text == intrinsics::SLEEP {
            if params.len() != 1 {
                diagnostics.report_error(
                    format!(
                        "'sleep' expects exactly 1 argument (milliseconds), got {}",
                        params.len()
                    ),
                    Some(name.position),
                );
            }
            for p in params {
                let pt = self.analyze_expression(p, parent_function, symbol_table, diagnostics)?;
                if !pt.is_int() {
                    diagnostics.report_error(
                        format!("'sleep' expects an int argument, got {}", pt.get_type()),
                        p.position(),
                    );
                }
            }
            return Ok(Type::Unknown);
        }

        // all/any/race take a single `Future<T>[]` argument.
        if params.len() != 1 {
            diagnostics.report_error(
                format!(
                    "'{}' expects exactly 1 argument (a Future array), got {}",
                    name.text,
                    params.len()
                ),
                Some(name.position),
            );
            return Ok(Type::Unknown);
        }
        let arg_type =
            self.analyze_expression(&params[0], parent_function, symbol_table, diagnostics)?;
        let inner_t = match &arg_type {
            Type::Array(inner) => match Self::future_inner_type(inner) {
                Some(t) => t,
                None => {
                    diagnostics.report_error(
                        format!(
                            "'{}' expects an array of Future values, got {}",
                            name.text,
                            arg_type.get_type()
                        ),
                        params[0].position(),
                    );
                    Type::Void
                }
            },
            _ => {
                diagnostics.report_error(
                    format!(
                        "'{}' expects an array of Future values, got {}",
                        name.text,
                        arg_type.get_type()
                    ),
                    params[0].position(),
                );
                Type::Void
            }
        };
        if name.text == intrinsics::PROMISE_ALL {
            // Future<T[]>
            Ok(Self::future_type(Type::Array(Box::new(inner_t))))
        } else {
            // any / race -> Future<T>
            Ok(Self::future_type(inner_t))
        }
    }


    /// Registers one monomorphized instance of a generic free function under its mangled name
    /// (`swap_int_string`, `natural_order_int`, ...), idempotently: a clone with its signature made
    /// concrete is stashed in `instantiated_generics` so its body is analyzed under `bindings`, and
    /// a matching signature is added to the function table. Shared by generic call resolution and by
    /// using a generic function as a first-class value. Returns the mangled name.
    pub(crate) fn register_generic_function_instance(
        &mut self,
        template: &'a FunctionNode<'a>,
        bindings: &GenericBindings,
    ) -> String {
        let mangled_name = mangle_bindings(&template.name.text, bindings);
        if self.function_table.get_function(&mangled_name).is_err() {
            // Store a clone with its signature monomorphized (params + return type made concrete),
            // mirroring how struct methods are specialized. The body is shared and resolved against
            // the bindings during analysis/codegen, so the declared return type (e.g. `List<T>` ->
            // `List_int`) stays consistent with what the body builds.
            let mut specialized = template.clone();
            Self::substitute_generic_signature(&mut specialized, bindings);
            let specialized_ref: &'a FunctionNode<'a> = self.arena.alloc(specialized);
            self.instantiated_generics
                .insert(mangled_name.clone(), (bindings.clone(), specialized_ref));

            let info = FunctionTableInfo {
                name: mangled_name.clone(),
                parameters: template
                    .parameters
                    .iter()
                    .map(|p| Self::monomorphize_type(&p.type_, bindings).get_type())
                    .collect(),
                defaults: template.parameters.iter().map(|p| p.default.clone()).collect(),
                return_type: template
                    .return_type
                    .as_ref()
                    .map(|ret| Self::monomorphize_type(ret, bindings)),
                is_async: template.is_async,
                is_static: template.is_static,
                is_public: template.is_public,
                intrinsic_name: intrinsics::intrinsic_key(&template.attributes),
            };

            let _ = self.function_table.add_function(mangled_name.clone(), info);
        }
        mangled_name
    }


    /// Instantiates a generic free function used as a first-class *value* (`let cmp: fun(T, T): int =
    /// natural_order;`). The concrete type arguments are inferred by unifying the template's declared
    /// parameter/return types with the `expected` function type at the use site; the instance is
    /// registered (see `register_generic_function_instance`) and a `FuncValue` referencing its
    /// mangled name is emitted. Returns the monomorphized function type, or `None` (with a
    /// diagnostic) if there is no function-typed context to infer from.
    pub(crate) fn instantiate_generic_function_value(
        &mut self,
        id: &SyntaxToken,
        diagnostics: &mut DiagnosticBag,
    ) -> Option<Type> {
        let template = *self.generic_functions.get(&id.text)?;

        // The expected type at this site drives inference; it must be a concrete function type.
        let expected = self
            .current_expected_type
            .as_ref()
            .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings));
        let Some(Type::Function(exp_params, exp_ret)) = expected else {
            diagnostics.report_error(
                format!(
                    "generic function '{}' can only be used as a value in a context with a known function type (e.g. `let f: fun(int, int): int = {};`)",
                    id.text, id.text
                ),
                Some(id.position),
            );
            return None;
        };

        // Infer bindings by matching the expected parameter types against the template's formals,
        // then verify the type parameters' constraints are satisfied by those concrete types.
        let param_strings: Vec<String> = exp_params.iter().map(|p| p.get_type()).collect();
        let bindings =
            self.infer_generic_bindings(template, &None, &param_strings, &id.position, diagnostics);
        self.verify_generic_constraints(
            &template.generic_constraints,
            &bindings,
            &id.position,
            diagnostics,
        );

        self.register_generic_function_instance(template, &bindings);
        // The func value must reference the base template's `DefId` + concrete instance args (in
        // binding order) so it maps to the monomorphized instance's function-table slot.
        let instance: Vec<crate::types::TypeId> = bindings
            .values()
            .map(|t| self.type_ctx.lower(t))
            .collect();
        let ret = (*exp_ret).clone();
        let func_ty = Type::Function(exp_params, exp_ret);
        self.hir_set_generic_func_value(&template.name.text, instance, &func_ty, &ret);
        Some(func_ty)
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
                let t = self.analyze_expression(&lit, parent_function, symbol_table, diagnostics)?;
                arg_hirs.push(self.hir_take());
                params_types.push(t.get_type());
            }
        }
        Ok(())
    }


}
