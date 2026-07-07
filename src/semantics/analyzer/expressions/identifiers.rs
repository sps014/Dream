//! Identifier resolution (locals, globals, first-class function values) and the name→`Type` parser.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::Type;
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(super) fn analyze_identifier(
        &mut self,
        id: &SyntaxToken,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let r = match (*symbol_table).as_ref().borrow().get_symbol(id) {
            Ok(t) => t,
            Err(e) => {
                // A bare identifier that names a top-level function is a first-class function value.
                if let Ok(sig) = self.function_table.get_function(&id.text) {
                    let params = sig
                        .parameters
                        .iter()
                        .map(|p| Self::type_from_name(p))
                        .collect();
                    let ret = sig.return_type.clone().unwrap_or(Type::Void);
                    let func_ty = Type::Function(params, Box::new(ret.clone()));
                    self.hir_set_func_value(&id.text, &func_ty, &ret);
                    return Ok(func_ty);
                }
                // A generic function used as a value (`let cmp: fun(T, T): int = natural_order;`):
                // infer its type arguments from the expected function type and instantiate it.
                if self.generic_functions.contains_key(&id.text) {
                    if let Some(func_ty) = self.instantiate_generic_function_value(id, diagnostics)
                    {
                        return Ok(func_ty);
                    }
                }
                // Unresolved name: report and short-circuit. Statement-level callers recover
                // (poisoning the binding with `Type::Unknown`) so sibling errors still surface.
                return Err(report(diagnostics, e.to_string(), Some(id.position)));
            }
        };
        // File/module-level visibility (Axis 2): a non-public top-level variable is only readable
        // from its declaring file. (Locals/params never appear in `self.globals`, so a shadowing
        // local of the same name is unaffected.)
        if let Some(global) = self.globals.iter().find(|g| g.name == id.text) {
            if !self.visible_across_files(
                &global.file_path,
                global.is_public,
                self.current_file.as_ref(),
            ) {
                let decl_file = global.file_path.clone();
                self.report_not_public("Variable", &id.text, &decl_file, id.position, diagnostics);
            }
        }
        self.hir_set_var(&id.text);
        Ok(r)
    }

    /// Reconstructs a `Type` from its canonical type-name string (as stored in function-table
    /// signatures), e.g. "int", "string", "Node", "int[]". Falls back to `void` if unparseable.
    pub(in crate::semantics::analyzer) fn type_from_name(name: &str) -> Type {
        let token = synthetic_token(TokenKind::IdentifierToken, name);
        Type::from_token(token).unwrap_or(Type::Void)
    }
}
