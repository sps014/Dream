//! `let` declarations, simple `name = value` assignments, and `return`.

use super::*;
use dream_diagnostics::DiagnosticBag;
use crate::errors::SemanticError;
use crate::symbol_table::SymbolTable;
use dream_syntax::nodes::{ExpressionNode, FunctionNode, Type};
use dream_syntax::token::syntax_token::SyntaxToken;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(in crate::analyzer) fn analyze_declaration(
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
        // array-typed (`int[]`) or `List<T>` annotation (e.g. `let xs: int[] = [];` / `let xs:
        // List<int> = [];`). With a valid annotation the literal is handled on the normal path
        // below (the annotation is published as the expected type, which the array-literal
        // analysis uses to allocate a zero-length array, or lower to `List<T>.from_array([])`).
        if let ExpressionNode::ArrayLiteral(elements) = right {
            if elements.is_empty()
                && !type_annotation.as_ref().is_some_and(|t| {
                    t.is_array() || Self::collection_generic_arg(t, "List").is_some()
                })
            {
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
        let mut value = self.hir_take();
        self.current_expected_type = saved_expected;

        let var_type = if let Some(t) = type_annotation {
            // A user-defined `@cast("implicit")` conversion is tried before the built-in
            // assignability check, so `let x: T = expr;` accepts it exactly like numeric widening.
            let converted_type;
            (converted_type, value) = self.apply_implicit_cast(&right_type, t, value);
            self.compare_data_type(t, &converted_type, &left.position, diagnostics)?;
            t.clone()
        } else {
            right_type.clone()
        };

        // A polymorphic generic function item is not a runtime value until instantiated at a use
        // site — bind it in the symbol table only (no HIR local).
        if matches!(var_type, Type::GenericFunctionItem(_)) {
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
            return Ok(());
        }

        self.record_capturing_fun_local(&left.text, &var_type, value.as_ref());
        self.hir_declare_local(&left.text, &var_type, value);
        self.hir_flush_ref_writebacks();

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

    /// `let (a, b) = expr;` — positional tuple destructure. When `expr` is a same-arity tuple
    /// literal, binds each name directly from the corresponding element (no materialized temp).
    /// Otherwise materializes the tuple and projects constant indices.
    pub(in crate::analyzer) fn analyze_tuple_declaration(
        &mut self,
        names: &[SyntaxToken],
        type_annotation: &Option<Type>,
        right: &ExpressionNode<'a>,
        is_const: bool,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        for name in names {
            self.check_reserved_name(name, "variable", diagnostics);
        }
        let mono_annotation = type_annotation
            .as_ref()
            .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings));
        let type_annotation = &mono_annotation;

        if let Some(t) = type_annotation {
            match t {
                Type::Tuple(elems) if elems.len() == names.len() => {}
                Type::Unknown => {}
                Type::Tuple(elems) => {
                    diagnostics.report_error(
                        format!(
                            "tuple type has {} elements but destructuring binds {}",
                            elems.len(),
                            names.len()
                        ),
                        names.first().map(|n| n.position),
                    );
                }
                other => {
                    diagnostics.report_error(
                        format!(
                            "tuple destructuring requires a tuple type, got {}",
                            other.display_name()
                        ),
                        names.first().map(|n| n.position),
                    );
                }
            }
        }

        // Fast path: `let (a, b) = (e0, e1);` — bind directly without a temp.
        if let ExpressionNode::TupleLiteral(elems) = right {
            if elems.len() == names.len() {
                let expected_elems: Option<Vec<Type>> = match type_annotation {
                    Some(Type::Tuple(ts)) => Some(ts.clone()),
                    _ => None,
                };
                for (i, (name, elem)) in names.iter().zip(elems.iter()).enumerate() {
                    let saved = self.current_expected_type.take();
                    self.current_expected_type =
                        expected_elems.as_ref().and_then(|es| es.get(i).cloned());
                    let elem_ty = self
                        .analyze_expression(elem, ctx.parent_function, ctx.symbol_table, diagnostics)
                        .unwrap_or(Type::Unknown);
                    let mut value = self.hir_take();
                    self.current_expected_type = saved;
                    let var_type = if let Some(es) = expected_elems.as_ref() {
                        let t = &es[i];
                        let converted;
                        (converted, value) = self.apply_implicit_cast(&elem_ty, t, value);
                        self.compare_data_type(t, &converted, &name.position, diagnostics)?;
                        t.clone()
                    } else {
                        elem_ty
                    };
                    self.record_capturing_fun_local(&name.text, &var_type, value.as_ref());
                    self.hir_declare_local(&name.text, &var_type, value);
                    if let Err(e) = (*ctx.symbol_table)
                        .as_ref()
                        .borrow_mut()
                        .add_symbol(name.text.clone(), var_type)
                    {
                        diagnostics.report_error(e.to_string(), Some(name.position));
                    }
                    if is_const {
                        (*ctx.symbol_table)
                            .as_ref()
                            .borrow_mut()
                            .mark_const(name.text.clone());
                    }
                }
                self.hir_flush_ref_writebacks();
                return Ok(());
            }
        }

        let saved_expected = self.current_expected_type.take();
        self.current_expected_type = type_annotation.clone();
        let right_type = self
            .analyze_expression(right, ctx.parent_function, ctx.symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let mut value = self.hir_take();
        self.current_expected_type = saved_expected;

        let tuple_ty = if let Some(t) = type_annotation {
            let converted;
            (converted, value) = self.apply_implicit_cast(&right_type, t, value);
            self.compare_data_type(t, &converted, &names[0].position, diagnostics)?;
            t.clone()
        } else {
            right_type.clone()
        };

        let Type::Tuple(elem_tys) = &tuple_ty else {
            if !right_type.is_unknown() {
                diagnostics.report_error(
                    format!(
                        "cannot destructure non-tuple type {}",
                        right_type.display_name()
                    ),
                    right.position(),
                );
            }
            self.hir_fail();
            return Ok(());
        };
        if elem_tys.len() != names.len() {
            diagnostics.report_error(
                format!(
                    "tuple has {} elements but destructuring binds {}",
                    elem_tys.len(),
                    names.len()
                ),
                names.first().map(|n| n.position),
            );
            self.hir_fail();
            return Ok(());
        }

        let temp_name = format!("__tuple_tmp_{}", names[0].position.start);
        self.hir_declare_local(&temp_name, &tuple_ty, value);
        if let Err(e) = (*ctx.symbol_table)
            .as_ref()
            .borrow_mut()
            .add_symbol(temp_name.clone(), tuple_ty.clone())
        {
            diagnostics.report_error(e.to_string(), names.first().map(|n| n.position));
        }

        for (i, name) in names.iter().enumerate() {
            let elem_ty = elem_tys[i].clone();
            self.hir_set_var(&temp_name);
            let base = self.hir_take();
            self.hir_set_field(base, i, &elem_ty);
            let field_val = self.hir_take();
            self.record_capturing_fun_local(&name.text, &elem_ty, field_val.as_ref());
            self.hir_declare_local(&name.text, &elem_ty, field_val);
            if let Err(e) = (*ctx.symbol_table)
                .as_ref()
                .borrow_mut()
                .add_symbol(name.text.clone(), elem_ty)
            {
                diagnostics.report_error(e.to_string(), Some(name.position));
            }
            if is_const {
                (*ctx.symbol_table)
                    .as_ref()
                    .borrow_mut()
                    .mark_const(name.text.clone());
            }
        }
        self.hir_flush_ref_writebacks();
        Ok(())
    }

    pub(in crate::analyzer) fn analyze_assignment(
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
        self.record_capturing_fun_local(&left.text, &l, value.as_ref());
        self.hir_assign_local(&left.text, value);
        Ok(())
    }
    pub(in crate::analyzer) fn analyze_return(
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
                let target = self.type_ctx.lower(return_type);
                self.hir_return_value(value, Some(target));
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
