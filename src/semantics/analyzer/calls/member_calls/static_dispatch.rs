//! `Type.method(args)` static/namespaced call analysis: discriminated-union variant construction,
//! on-the-fly monomorphization of generic static methods (including `Buffer.alloc`/`Bytes.of`/`to`,
//! `Promise.*` combinators, and `JSON.serialize`/`deserialize`), and plain static-method resolution.

use super::super::super::*;
use crate::diagnostics::DiagnosticBag;
use crate::intrinsics;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::FunctionTableInfo;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::{
    canonical_type_name, is_numeric_primitive, is_unknown_type_name, mangle_generic,
};
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::types::method_fn;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
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
        ctx: &super::super::super::AnalyzerContext<'a, '_>,
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
        if let Some(&template) = self.generic_functions.get(&base) {
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
                    None => params_types
                        .first()
                        .map(|s| named(s))
                        .unwrap_or(Type::Unknown),
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
