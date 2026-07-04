//! Analysis of call expressions: free-function and overload resolution, method calls, static /
//! namespaced calls (`Math.*` / `JSON.*` / async intrinsics / `derive` helpers), and constructors.

use super::super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::{FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::types::constructor_fn;
use std::cell::RefCell;
use std::rc::Rc;

/// Outcome of looking up an indexer/enumerator "hook" method (`get`/`set`/`iterator`/`next`) on a
/// struct receiver, for the desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.

impl<'a> Analyzer<'a> {
    /// Type-checks a constructor call `Struct(args)`. When the struct defines a custom `constructor`
    /// the call is checked against `init`'s parameters; otherwise the class has an implicit zero-arg
    /// default constructor (`Struct()`) that leaves every field at its zero value.
    pub(crate) fn analyze_constructor_call(
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
            self.validate_arguments(
                &format!("Constructor for '{}'", struct_name),
                &expected,
                params_types,
                name.position,
                diagnostics,
            );
        }

        Ok(Type::Struct(
            synthetic_token(TokenKind::IdentifierToken, &struct_name),
            None,
        ))
    }
}
