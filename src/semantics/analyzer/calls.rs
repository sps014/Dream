//! Analysis of call expressions: free-function and overload resolution, method calls, static /
//! namespaced calls (`Math.*` / `JSON.*` / async intrinsics / `derive` helpers), and constructors.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::intrinsics;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::{FunctionTableInfo, OverloadResolution};
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::{
    canonical_type_name, is_numeric_primitive, is_unknown_type_name, mangle_generic, strip_nullable,
};
use crate::types::{constructor_fn, method_fn};
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

/// Outcome of looking up an indexer/enumerator "hook" method (`get`/`set`/`iterator`/`next`) on a
/// struct receiver, for the desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.
pub(super) enum HookResolution {
    /// The receiver is not a struct, or it has no method with that name: the sugar is unavailable.
    Absent,
    /// A method with that name exists but cannot serve as a hook; carries a human-readable reason
    /// (it is static, async, or has the wrong number of parameters).
    Ineligible(String),
    /// A usable hook: an accessible instance, non-async method with the requested declared arity.
    Eligible(FunctionTableInfo),
}

impl<'a> Analyzer<'a> {
    /// Resolves a hook method named `method_name` (with declared arity `declared_arity`, i.e.
    /// excluding the implicit `this`) on struct receiver `obj_type`, ensuring the receiver's generic
    /// instance is registered first. Return-type shape checks (non-void for `get`, `Option<T>` for
    /// `next`, etc.) are left to the caller. An overloaded hook resolves to the first overload that
    /// matches the requested arity. A same-named method that is `static`, `async`, or of the wrong
    /// arity yields `Ineligible` (so `obj[i]`/`for..in` never silently hijack an ordinary method),
    /// while a call like `obj.get(i)` keeps resolving through the normal method path.
    pub(super) fn resolve_hook_method(
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

    /// Resolves an overloaded base name against the concrete `arg_types`, returning the selected
    /// signature or a human-readable error (no match / ambiguous). Used by both free-function and
    /// method call analysis (methods prepend the receiver type as the implicit `this` argument).
    pub(super) fn select_function_overload(
        &mut self,
        base: &str,
        arg_types: &[String],
    ) -> Result<FunctionTableInfo, String> {
        // Overload viability is a structural relation over interned ids (object widening, enum/int,
        // numeric, nullable): lower each spelling and defer to `types::overload_compatible` rather
        // than re-deriving the rules from strings.
        let type_ctx = &mut self.type_ctx;
        let compat = |param: &str, arg: &str| {
            let p = type_ctx.lower_str(param);
            let a = type_ctx.lower_str(arg);
            crate::types::overload_compatible(&type_ctx.interner, p, a)
        };
        match self.function_table.select_overload(base, arg_types, compat) {
            OverloadResolution::Unique(key) => Ok(self.function_table.get_function(&key).unwrap()),
            OverloadResolution::None => Err(format!(
                "No overload of '{}' matches argument types ({})",
                base,
                arg_types.join(", ")
            )),
            OverloadResolution::Ambiguous(keys) => Err(format!(
                "Ambiguous call to '{}' with argument types ({}); candidates: {}",
                base,
                arg_types.join(", "),
                keys.join(", ")
            )),
        }
    }

    /// Analyzes a static-method call `Type.method(args)` (resolved by the caller to the type
    /// `type_name`). Static methods have no implicit `this`, so the explicit arguments map 1:1 to
    /// the declared parameters.
    pub(super) fn analyze_static_call(
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
            return Ok(store_sig.return_type.unwrap_or(Type::Void));
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
        // `Future<T>` that must be `await`ed, just like an async instance method or free function.
        // An `async` static method yields a `Future<T>` handle (carried by the `Call`); `await`
        // unwraps it.
        let ret_type = if store_sig.is_async {
            Self::future_type(store_sig.return_type.unwrap_or(Type::Void))
        } else {
            store_sig.return_type.unwrap_or(Type::Void)
        };
        // A static method is a free function under its mangled `{Type}_{method}` name (no receiver);
        // overloaded names are ambiguous for a single `DefId` lookup, so defer those.
        if self.function_table.is_overloaded(&base) {
            self.hir_none();
        } else {
            self.hir_set_call(&base, arg_hirs, &ret_type);
        }
        Ok(ret_type)
    }

    /// True when `parent_function` is a method whose implicit `this` receiver has base type
    /// `base_name` (allowing for monomorphized generic variants). Used to gate access to
    /// `_`-prefixed (private) members.
    pub(super) fn in_methods_of(
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

    /// Appends the default values of any omitted trailing parameters to a call's argument lists.
    /// `defaults` is the callee's per-parameter default slice (parallel to its parameters); for each
    /// index at or past the number of supplied arguments that carries a default, its constant
    /// literal is analyzed exactly like an explicit literal argument, extending both `params_types`
    /// (for the per-index type check) and `arg_hirs` (for the emitted call). Callers must have
    /// already validated arity (supplied count within `required..=total`).
    fn substitute_default_args(
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

    pub(super) fn analyze_function_call(
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
            for i in 0..param_types.len() {
                let expected = param_types[i].get_type();
                if expected != "object" && expected != params_types[i] {
                    diagnostics.report_error(
                        format!(
                            "function value '{}' expects argument {} to be {}, got {}",
                            name.text,
                            i + 1,
                            expected,
                            params_types[i]
                        ),
                        Some(name.position),
                    );
                }
            }
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
            let mangled_name = mangle_bindings(&function_name, &bindings);
            generic_instance = Some((
                function_name.clone(),
                bindings.values().cloned().collect(),
            ));

            if self.function_table.get_function(&mangled_name).is_err() {
                // Store a clone with its signature monomorphized (params + return type made
                // concrete), mirroring how struct methods are specialized. The body is shared and
                // resolved against the bindings during analysis/codegen, so the declared return
                // type (e.g. `List<T>` -> `List_int`) stays consistent with what the body builds.
                let mut specialized = template.clone();
                Self::substitute_generic_signature(&mut specialized, &bindings);
                let specialized_ref: &'a FunctionNode<'a> = self.arena.alloc(specialized);
                self.instantiated_generics
                    .insert(mangled_name.clone(), (bindings.clone(), specialized_ref));

                let info = FunctionTableInfo {
                    name: mangled_name.clone(),
                    parameters: template
                        .parameters
                        .iter()
                        .map(|p| Self::monomorphize_type(&p.type_, &bindings).get_type())
                        .collect(),
                    defaults: template.parameters.iter().map(|p| p.default.clone()).collect(),
                    return_type: template
                        .return_type
                        .as_ref()
                        .map(|ret| Self::monomorphize_type(ret, &bindings)),
                    is_async: template.is_async,
                    is_static: template.is_static,
                    is_public: template.is_public,
                    intrinsic_name: intrinsics::intrinsic_key(&template.attributes),
                };

                let _ = self.function_table.add_function(mangled_name.clone(), info);
            }
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
            return Ok(Type::Void);
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

        for i in 0..params_types.len() {
            // A parameter declared `object` accepts any argument type (boxing happens in codegen).
            if store_sig
                .parameters
                .get(i)
                .map(|s| s == "object")
                .unwrap_or(false)
                || params_types
                    .get(i)
                    .map(|s| is_unknown_type_name(s))
                    .unwrap_or(false)
            {
                continue;
            }
            if store_sig.parameters.get(i) != params_types.get(i) {
                let expected = store_sig
                    .parameters
                    .get(i)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let given = params_types.get(i).map(|s| s.as_str()).unwrap_or("");
                if self.enum_int_compatible(expected, given) {
                    continue;
                }
                // Implicit upcast: a class argument is accepted where an interface it implements is
                // expected (e.g. passing a `Cat` to a `fun(a: Animal)` parameter).
                if self.is_interface_name(strip_nullable(expected))
                    && self.implements_as_interface_ref(
                        strip_nullable(given),
                        strip_nullable(expected),
                    )
                {
                    continue;
                }
                diagnostics.report_error(
                    format!(
                        "Function {} has param {} of type {:?} but param {} of type {:?} is given",
                        function_name,
                        i,
                        store_sig.parameters.get(i),
                        i,
                        params_types[i]
                    ),
                    Some(name.position),
                );
            }
        }

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
    pub(super) fn analyze_async_intrinsic(
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
            return Ok(Self::future_type(Type::Void));
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
            return Ok(Self::future_type(Type::Void));
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

    /// String-level assignability check for argument vs. parameter/field types, mirroring the
    /// rules in [`compare_data_type`] (which works on `Type`). An `expected` type accepts a `given`
    /// when they are identical, the target is `object`, they are enum/int compatible, or the target
    /// is nullable (`T?`) and the argument is `T`, `T?`, or the `null` literal (`void?`). Used by
    /// constructor-call checking, which only has the type names (not structured `Type`s) available.
    pub(super) fn type_str_assignable(&mut self, expected: &str, given: &str) -> bool {
        // The poison type unifies with everything so an earlier error never cascades into a
        // spurious assignment/argument mismatch here. (Kept as an explicit name check because the
        // unknown spelling has no dedicated interned id.)
        if crate::syntax::nodes::types::is_unknown_type_name(expected)
            || crate::syntax::nodes::types::is_unknown_type_name(given)
        {
            return true;
        }
        // Directional assignability over interned types: `given` (value) must be assignable to
        // `expected` (target). Covers identity, `object` widening, enum/int, numeric widening, and
        // nullable/`null` handling via the structured rules.
        let e = self.type_ctx.lower_str(expected);
        let g = self.type_ctx.lower_str(given);
        if crate::types::assignable(&self.type_ctx.interner, e, g) {
            return true;
        }
        // Implicit upcast to an interface parameter: the argument's concrete class implements it.
        let iface = crate::syntax::nodes::types::strip_nullable(expected);
        if self.is_interface_name(iface) {
            let given_class = crate::syntax::nodes::types::strip_nullable(given);
            return self.implements_as_interface_ref(given_class, iface);
        }
        false
    }

    /// Type-checks a constructor call `Struct(args)`. When the struct defines a custom `constructor`
    /// the call is checked against `init`'s parameters; otherwise the class has an implicit zero-arg
    /// default constructor (`Struct()`) that leaves every field at its zero value.
    pub(super) fn analyze_constructor_call(
        &mut self,
        name: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params_types: &mut Vec<String>,
        arg_hirs: &mut Vec<Option<crate::hir::HExpr>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let struct_name = match generic_args {
            Some(args) if !args.is_empty() => {
                self.ensure_struct_instantiated(&name.text, args, &name.position, diagnostics);
                mangle_generic(&name.text, args)
            }
            _ => {
                if self.generic_structs.contains_key(&name.text) {
                    diagnostics.report_error(
                        format!(
                            "Generic class '{}' requires type arguments, e.g. {}<int>(...)",
                            name.text, name.text
                        ),
                        Some(name.position),
                    );
                }
                name.text.clone()
            }
        };

        let init_name = constructor_fn(&struct_name);
        // `expected` are the constructor's parameter types (a user `constructor` skips its implicit
        // `this`); `expected_defaults` are the parallel default values. A class with no explicit
        // `constructor` has an implicit zero-arg default constructor, so it expects no arguments.
        let (expected, expected_defaults): (Vec<String>, Vec<Option<Type>>) =
            if let Ok(sig) = self.function_table.get_function(&init_name) {
                // `constructor` is registered as a method, so parameter 0 is the implicit `this`.
                (
                    sig.parameters.iter().skip(1).cloned().collect(),
                    sig.defaults.iter().skip(1).cloned().collect(),
                )
            } else {
                (Vec::new(), Vec::new())
            };

        let required = expected_defaults
            .iter()
            .position(|d| d.is_some())
            .unwrap_or(expected.len());
        let total = expected.len();
        let given = params_types.len();
        if given < required || given > total {
            let message = if required == total {
                format!(
                    "Constructor for '{}' expects {} argument(s), but {} were given",
                    struct_name, total, given
                )
            } else {
                format!(
                    "Constructor for '{}' expects between {} and {} argument(s), but {} were given",
                    struct_name, required, total, given
                )
            };
            diagnostics.report_error(message, Some(name.position));
        } else {
            // Fill omitted trailing arguments with their defaults (extends both the type list and
            // the emitted argument HIR so the generated `New` receives the complete argument set).
            self.substitute_default_args(
                &expected_defaults,
                params_types,
                arg_hirs,
                parent_function,
                symbol_table,
                diagnostics,
            )?;
            for i in 0..expected.len() {
                let e = expected[i].as_str();
                let g = params_types[i].as_str();
                if self.type_str_assignable(e, g) {
                    continue;
                }
                diagnostics.report_error(
                    format!(
                        "Constructor for '{}' expects argument {} to be '{}', got '{}'",
                        struct_name,
                        i + 1,
                        e,
                        g
                    ),
                    Some(name.position),
                );
            }
        }

        Ok(Type::Struct(
            synthetic_token(TokenKind::IdentifierToken, &struct_name),
            None,
        ))
    }

    pub(super) fn analyze_method_call(
        &mut self,
        obj: &ExpressionNode<'a>,
        method: &SyntaxToken,
        _generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::AnalyzerContext<'a, '_>,
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
    /// static methods (including the `Array.new` and promise-combinator intrinsics), and plain
    /// static-method resolution. Returns `Ok(Some(type))` when handled, `Ok(None)` when `id` is a
    /// local or names no static member (so the caller falls through to instance dispatch).
    fn try_analyze_static_method(
        &mut self,
        id: &SyntaxToken,
        method: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::AnalyzerContext<'a, '_>,
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
                return Ok(Some(Type::Void));
            }
            // Generic static calls / intrinsics need an `InstanceId` (a later slice); stay out of
            // HIR coverage regardless of which sub-branch handles the call.
            self.hir_none();
            // `Array.new<T>(len)`: a generic intrinsic that allocates a zero-initialized
            // `T[]`. The element type comes from the explicit type argument (resolved
            // through the active monomorphization bindings so `Array.new<T>` inside a
            // `List<int>` method yields `int[]`).
            if intrinsics::IntrinsicOp::from_attributes(&template.attributes)
                == Some(intrinsics::IntrinsicOp::ArrayNew)
            {
                let element = match generic_args.as_ref().and_then(|g| g.first()) {
                    Some(t) => Self::monomorphize_type(t, &self.current_generic_bindings),
                    None => {
                        diagnostics.report_error(
                            "'Array.new' requires a type argument, e.g. Array.new<int>(n)"
                                .to_string(),
                            Some(method.position),
                        );
                        Type::Void
                    }
                };
                if params_types.len() != 1 {
                    diagnostics.report_error(
                        format!(
                            "'Array.new' expects exactly 1 argument (length), got {}",
                            params_types.len()
                        ),
                        Some(method.position),
                    );
                } else if params_types[0] != "int" && !is_unknown_type_name(&params_types[0]) {
                    diagnostics.report_error(
                        format!("'Array.new' length must be int, got {}", params_types[0]),
                        Some(method.position),
                    );
                }
                self.hir_set_array_new(&element, arg_hirs.into_iter().next().flatten());
                return Ok(Some(Type::Array(Box::new(element))));
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
                self.hir_set_call(&method_fn(&struct_name, "to_json"), vec![value], &named("JsonValue"));
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

            if self.function_table.get_function(&mangled_name).is_err() {
                let mut specialized = template.clone();
                Self::substitute_generic_signature(&mut specialized, &bindings);
                let specialized_ref: &'a FunctionNode<'a> = self.arena.alloc(specialized);
                let info = FunctionTableInfo::from(specialized_ref);
                self.function_table
                    .add_function(mangled_name.clone(), info)
                    .unwrap();
                self.instantiated_generics
                    .insert(mangled_name.clone(), (bindings, specialized_ref));
            }
            let info = self.function_table.get_function(&mangled_name).unwrap();
            if info.is_async {
                return Ok(Some(Self::future_type(info.return_type.unwrap_or(Type::Void))));
            }
            return Ok(Some(info.return_type.unwrap_or(Type::Void)));
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
    fn analyze_builtin_method(
        &mut self,
        obj_type: &Type,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::AnalyzerContext<'a, '_>,
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
    fn interface_receiver_name(&self, obj_type: &Type) -> Option<String> {
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
    fn analyze_interface_method(
        &mut self,
        iface_name: &str,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::AnalyzerContext<'a, '_>,
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

        let iface_id = self
            .interface_methods
            .get_index_of(iface_name)
            .unwrap_or(0);
        // The `call_indirect` signature is `fun(this, params...): ret`, with `this` typed as
        // `object` (an `i32` pointer, matching every concrete implementation's receiver).
        let sig = self.interface_dispatch_sig(im);
        self.hir_set_interface_call(receiver, iface_id, slot, sig, arg_hirs, &ret_type);
        Ok(ret_type)
    }

    /// Interns the `fun(this, params...): ret` function type used to `call_indirect` an interface
    /// method: `this` is `object` (a tagged pointer), followed by the method's declared parameters
    /// and its return type. The same signature is used to declare the WASM `call_indirect` type.
    pub(super) fn interface_dispatch_sig(&mut self, method: &FunctionNode<'a>) -> crate::types::TypeId {
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

    fn analyze_instance_method(
        &mut self,
        obj_type: &Type,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::AnalyzerContext<'a, '_>,
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
            return Ok(store_sig.return_type.unwrap_or(Type::Void));
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

        for (i, given_type) in arg_types.iter().enumerate() {
            let expected_type_str = &expected_params[i];

            if expected_type_str == "object" || is_unknown_type_name(given_type) {
                continue;
            }

            if is_numeric_primitive(expected_type_str) && is_numeric_primitive(given_type) {
                continue;
            }

            if given_type != expected_type_str {
                diagnostics.report_error(
                    format!(
                        "function {} expects parameter {} to be {}, got {}",
                        mangled_name,
                        i + 1,
                        expected_type_str,
                        given_type
                    ),
                    Some(method.position),
                );
            }
        }

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
}
