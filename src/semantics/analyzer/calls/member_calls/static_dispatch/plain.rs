//! Plain (non-generic) static-method resolution: `analyze_static_call`.

use super::*;
use crate::syntax::nodes::types::{is_numeric_primitive, is_unknown_type_name};

impl<'a> Analyzer<'a> {
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
        let is_overloaded = self.function_table.is_overloaded(&base);

        // When the callee isn't overloaded, its declared parameter types are already known before
        // the arguments are analyzed, so publish them as `current_expected_type` per argument
        // (mirroring the free-function call path) — needed, e.g., for an empty array-literal
        // argument to infer its element type from a `T[]` parameter rather than requiring its own
        // annotation. An overloaded callee can't do this (the signature isn't known until the
        // arguments are typed), so it falls back to no expected-type context, as before.
        let expected_params: Option<Vec<Type>> = if is_overloaded {
            None
        } else {
            self.function_table
                .get_function(&base)
                .ok()
                .map(|s| Self::expected_param_types(&s))
        };

        let call_target = format!("{}.{}", type_name, method.text);
        let saved_call_target = self.current_call_target_name.take();
        self.current_call_target_name = Some(call_target);

        let (arg_types, arg_hirs, arg_is_ref) = self.analyze_call_arguments_expecting_ref(
            params,
            expected_params.as_deref(),
            parent_function,
            symbol_table,
            diagnostics,
        )?;

        self.current_call_target_name = saved_call_target;

        let store_sig = if is_overloaded {
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

        if !self.member_accessible(
            store_sig.visibility,
            &store_sig.declaring_file,
            parent_function.file_path.as_ref(),
            self.in_methods_of(parent_function, type_name),
        ) {
            diagnostics.report_error(
                format!("'{}' is private to '{}'", method.text, type_name),
                Some(method.position),
            );
        }

        self.check_unsafe_call(&store_sig, method.position, diagnostics);

        self.validate_ref_arguments(
            &format!("static method '{}'", base),
            &store_sig.is_ref,
            &arg_is_ref,
            method.position,
            diagnostics,
        );

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
        let ret_type = Self::async_return_type(store_sig.is_async, store_sig.return_type);
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
