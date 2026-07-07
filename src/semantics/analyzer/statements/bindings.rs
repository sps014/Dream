//! `let` declarations, simple `name = value` assignments, and `return`.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(in crate::semantics::analyzer) fn analyze_declaration(
        &mut self,
        left: &SyntaxToken,
        type_annotation: &Option<Type>,
        right: &ExpressionNode<'a>,
        is_const: bool,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        self.check_reserved_name(left, "variable", diagnostics);
        // Inside a monomorphized generic body, substitute the type parameters in the annotation with
        // their concrete types (e.g. `let cmp: fun(T, T): int` becomes `fun(int, int): int`), so the
        // published expected type, the initializer check, and the recorded variable type are all
        // concrete. Outside a generic body the bindings are empty and this just clones.
        let mono_annotation = type_annotation
            .as_ref()
            .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings));
        let type_annotation = &mono_annotation;
        // Empty array literals carry no element type, so the declaration must supply one via an
        // array-typed annotation (e.g. `let xs: int[] = [];`). With a valid annotation the literal is
        // handled on the normal path below (the annotation is published as the expected type, which
        // the array-literal analysis uses to allocate a zero-length array).
        if let ExpressionNode::ArrayLiteral(elements) = right {
            if elements.is_empty() && !type_annotation.as_ref().is_some_and(|t| t.is_array()) {
                self.hir_fail();
                diagnostics.report_error(
                    "cannot infer the element type of an empty array literal; add an array type annotation, e.g. `let xs: int[] = [];`".to_string(),
                    Some(left.position),
                );
                return Ok(());
            }
        }
        //return right type. A type annotation is published as the expected type so a generic
        // union's nullary variant (`let o: Option<int> = Option.None;`) can resolve its arguments.
        let saved_expected = self.current_expected_type.take();
        self.current_expected_type = type_annotation.clone();
        // Recover at the binding site: even when the initializer short-circuits, fall back to the
        // poison type so the variable is still registered (with its annotated type, if any) and
        // later uses of it don't spuriously report "does not exist".
        let right_type = self
            .analyze_expression(right, ctx.parent_function, ctx.symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let value = self.hir_take();
        self.current_expected_type = saved_expected;

        let var_type = if let Some(t) = type_annotation {
            self.compare_data_type(t, &right_type, &left.position, diagnostics)?;
            t.clone()
        } else {
            right_type.clone()
        };

        self.hir_declare_local(&left.text, &var_type, value);

        if let Err(e) = (*ctx.symbol_table)
            .as_ref()
            .borrow_mut()
            .add_symbol(left.text.clone(), var_type)
        {
            diagnostics.report_error(e.to_string(), Some(left.position));
        }
        if is_const {
            (*ctx.symbol_table)
                .as_ref()
                .borrow_mut()
                .mark_const(left.text.clone());
        }
        Ok(())
    }
    pub(in crate::semantics::analyzer) fn analyze_assignment(
        &mut self,
        left: &SyntaxToken,
        right: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        if (*symbol_table).as_ref().borrow().is_const(&left.text) {
            diagnostics.report_error(
                format!(
                    "Cannot assign to '{}' because it is a const binding",
                    left.text
                ),
                Some(left.position),
            );
        }
        // Peek the target's declared type first so it can drive inference of the right-hand side
        // (e.g. an untyped empty array literal `xs = []` resolves to the variable's element type).
        let l = match (*symbol_table).as_ref().borrow().get_symbol(left) {
            Ok(sym) => sym,
            Err(e) => {
                diagnostics.report_error(e.to_string(), Some(left.position));
                self.hir_fail();
                return Ok(());
            }
        };
        let saved_expected = self.current_expected_type.take();
        self.current_expected_type = Some(l.clone());
        let r = self
            .analyze_expression(right, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let value = self.hir_take();
        self.current_expected_type = saved_expected;
        self.compare_data_type(&l, &r, &left.position, diagnostics)?;
        self.hir_assign_local(&left.text, value);
        Ok(())
    }
    pub(in crate::semantics::analyzer) fn analyze_return(
        &mut self,
        expression: &Option<ExpressionNode<'a>>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        match (expression, &parent_function.return_type) {
            (Some(expression), Some(return_type)) => {
                let saved_expected = self.current_expected_type.take();
                self.current_expected_type = Some(return_type.clone());
                let r = self
                    .analyze_expression(expression, parent_function, symbol_table, diagnostics)
                    .unwrap_or(Type::Unknown);
                let value = self.hir_take();
                self.current_expected_type = saved_expected;
                self.compare_data_type(
                    return_type,
                    &r,
                    &parent_function.name.position,
                    diagnostics,
                )?;
                self.hir_return_value(value);
            }
            // A bare `return;` is allowed in a `void` function (an explicit `: void` annotation
            // parses to `Some(Type::Void)`, which is semantically the same as an unannotated
            // function); it exits early with no value.
            (None, Some(Type::Void)) => self.hir_return_void(),
            (None, &Some(_)) => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "return type mismatch at  {}",
                        parent_function.name.position.get_point_str()
                    ),
                    Some(parent_function.name.position),
                );
            }
            (Some(_), &None) => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "return type mismatch at {}",
                        parent_function.name.position.get_point_str()
                    ),
                    Some(parent_function.name.position),
                );
            }
            (None, &None) => self.hir_return_void(),
        };
        Ok(())
    }
}
