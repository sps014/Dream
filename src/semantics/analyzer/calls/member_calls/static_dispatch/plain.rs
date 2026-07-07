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

        let (arg_types, arg_hirs) =
            self.analyze_call_arguments(params, parent_function, symbol_table, diagnostics)?;

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
