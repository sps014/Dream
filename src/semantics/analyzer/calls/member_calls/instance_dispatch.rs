//! Ordinary and interface instance-method resolution once the receiver type is known and the
//! static/builtin cases have been ruled out.

use super::super::super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::syntax::nodes::types::{mangle_generic, strip_nullable};
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::types::method_fn;

impl<'a> Analyzer<'a> {
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
        ctx: &super::super::super::AnalyzerContext<'a, '_>,
        receiver: Option<crate::hir::HExpr>,
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

    /// Resolves and type-checks an instance method call `obj.method(args)` once the receiver type
    /// (`obj_type`) is known and the builtins/static cases have been ruled out: monomorphizes the
    /// receiver, selects the (possibly overloaded) `{Type}_{method}`, enforces privacy and the
    /// argument arity/types, and returns the call's result type (a `Future<T>` for `async`).
    pub(crate) fn analyze_instance_method(
        &mut self,
        obj_type: &Type,
        method: &SyntaxToken,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::super::AnalyzerContext<'a, '_>,
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
        let (mut arg_types, mut arg_hirs) = self.analyze_call_arguments(
            params,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;

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
}
