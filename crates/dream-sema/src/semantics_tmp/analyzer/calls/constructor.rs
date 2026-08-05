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

impl<'a> Analyzer<'a> {
    /// Type-checks a constructor call `Struct(args)`. When the struct defines a custom `constructor`
    /// the call is checked against `init`'s parameters; otherwise the class has an implicit zero-arg
    /// default constructor (`Struct()`) that leaves every field at its zero value. A struct may
    /// declare more than one `constructor` overload (registered like any other method via
    /// `add_overload`); the matching one is selected by argument count/types exactly like an
    /// overloaded free function or method call. Returns the selected constructor's *emitted*
    /// (possibly signature-mangled) name alongside the constructed type, so the caller resolves the
    /// `New` HIR node's `DefId` to the actual overload chosen here rather than re-deriving the bare
    /// `{struct}_constructor` name (which is ambiguous once there is more than one overload).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn analyze_constructor_call(
        &mut self,
        name: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params_types: &mut Vec<String>,
        arg_hirs: &mut Vec<Option<crate::hir::HExpr>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(Type, Option<String>), SemanticError> {
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

        // File/module-level visibility (Axis 2): a non-public class is only constructible from its
        // own file.
        if let Some(info) = self.struct_table.get_struct(&struct_name) {
            if !self.visible_across_files(
                &info.file_path,
                info.visibility,
                parent_function.file_path.as_ref(),
            ) {
                let decl_file = info.file_path.clone();
                self.report_not_public("Class", &name.text, &decl_file, name.position, diagnostics);
            }
        }

        let init_name = constructor_fn(&struct_name);
        // A struct with more than one `constructor` overload is resolved exactly like an
        // overloaded free function/method: the implicit `this` (the struct itself) plus the given
        // argument types are matched against every registered overload's full parameter list.
        let resolved_ctor: Option<crate::semantics::function_table::FunctionTableInfo> =
            if self.function_table.is_overloaded(&init_name) {
                let mut selection_args = Vec::with_capacity(params_types.len() + 1);
                selection_args.push(struct_name.clone());
                selection_args.extend(params_types.iter().cloned());
                match self.select_function_overload(&init_name, &selection_args) {
                    Ok(sig) => Some(sig),
                    Err(message) => {
                        diagnostics.report_error(message, Some(name.position));
                        None
                    }
                }
            } else {
                self.function_table.get_function(&init_name).ok()
            };
        // `expected` are the constructor's parameter types (a user `constructor` skips its implicit
        // `this`); `expected_defaults` are the parallel default values. A class with no explicit
        // `constructor` has an implicit zero-arg default constructor, so it expects no arguments.
        // An overloaded constructor whose resolution already failed above skips the redundant
        // arity/type re-check below (its own error was already reported).
        let overload_resolution_failed =
            resolved_ctor.is_none() && self.function_table.is_overloaded(&init_name);
        let (expected, expected_defaults): (Vec<String>, Vec<Option<Type>>) = match &resolved_ctor {
            // `constructor` is registered as a method, so parameter 0 is the implicit `this`.
            Some(sig) => (
                sig.parameters.iter().skip(1).cloned().collect(),
                sig.defaults.iter().skip(1).cloned().collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };

        if !overload_resolution_failed {
            let total = expected.len();
            let required = Self::required_arg_count(&expected_defaults, total);
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
        }

        Ok((
            Type::Struct(
                synthetic_token(TokenKind::IdentifierToken, &struct_name),
                None,
            ),
            resolved_ctor.map(|sig| sig.name),
        ))
    }
}
