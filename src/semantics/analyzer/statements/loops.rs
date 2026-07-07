//! `break`/`continue` placement checks and the `while`/`do-while`/`for`/`for-each` loop forms.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, StatementNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(in crate::semantics::analyzer) fn analyze_break(
        &mut self,
        label: &Option<String>,
        parent_function: &FunctionNode<'a>,
        has_parent_while: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        if !has_parent_while {
            diagnostics.report_error(
                format!(
                    "Break statement is not in a loop in function {}",
                    parent_function.name.text
                ),
                Some(parent_function.name.position),
            );
        }
        if let Some(name) = label {
            if !self.loop_labels.contains(name) {
                diagnostics.report_error(
                    format!("Break targets unknown loop label '{}'", name),
                    Some(parent_function.name.position),
                );
            }
        }
        self.hir_break(label.clone());
        Ok(())
    }
    pub(in crate::semantics::analyzer) fn analyze_continue(
        &mut self,
        label: &Option<String>,
        parent_function: &FunctionNode<'a>,
        has_parent_while: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        if !has_parent_while {
            diagnostics.report_error(
                format!(
                    "Continue statement is not in a loop in function {}",
                    parent_function.name.text
                ),
                Some(parent_function.name.position),
            );
        }
        if let Some(name) = label {
            if !self.loop_labels.contains(name) {
                diagnostics.report_error(
                    format!("Continue targets unknown loop label '{}'", name),
                    Some(parent_function.name.position),
                );
            }
        }
        self.hir_continue(label.clone());
        Ok(())
    }
    pub(in crate::semantics::analyzer) fn analyze_foreach(
        &mut self,
        statement: &StatementNode<'a>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let StatementNode::ForEach(element, iterable, index_name, array_name, body) = statement
        else {
            unreachable!()
        };
        let iterable_type = self
            .analyze_expression(iterable, ctx.parent_function, ctx.symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let iter_hir = self.hir_take();

        // A class or `string` receiver iterates through the enumerator protocol (`iterator()` ->
        // `next()`), lowered directly to a `while` loop (see `analyze_foreach_iter`). `string`
        // exposes `iterator()` via `extend string`, so `for (let c in s)` walks its chars. Arrays
        // keep the built-in index loop below.
        if !matches!(iterable_type, Type::Array(_))
            && (Self::resolve_struct_parts(&iterable_type).is_some()
                || matches!(iterable_type, Type::String(_)))
        {
            return self.analyze_foreach_iter(
                element,
                &iterable_type,
                iter_hir,
                body,
                ctx,
                diagnostics,
            );
        }

        let element_type = match &iterable_type {
            Type::Array(inner) => (**inner).clone(),
            // Don't cascade a fresh error if the base was already poisoned by an earlier one.
            Type::Unknown => Type::Unknown,
            _ => {
                diagnostics.report_error(
                    format!(
                        "for-each can only iterate over arrays or types with an 'iterator()' method, got {}",
                        iterable_type.get_type()
                    ),
                    iterable.position(),
                );
                Type::Void
            }
        };

        // Claim any label wrapping this loop before its body (which may hold nested loops) is analyzed.
        let label = self.pending_loop_label.take();
        // Register the synthetic loop locals plus the user's element binding in a dedicated scope.
        let foreach_scope = Rc::new(RefCell::new(SymbolTable::new(Some(
            ctx.symbol_table.clone(),
        ))));
        (*ctx.symbol_table)
            .borrow_mut()
            .add_child(foreach_scope.clone());
        {
            let mut scope = (*foreach_scope).borrow_mut();
            let _ = scope.add_symbol(array_name.to_string(), iterable_type.clone());
            let _ = scope.add_symbol(
                index_name.to_string(),
                Type::Integer(synthetic_token(TokenKind::DataTypeToken, "int")),
            );
            if let Err(e) = scope.add_symbol(element.text.clone(), element_type.clone()) {
                diagnostics.report_error(e.to_string(), Some(element.position));
            }
        }
        // Allocate the element slot before the body so body references resolve to it. The synthetic
        // index/array locals are internal to the MIR `Foreach` lowering and get no HIR slot, so a
        // body that reads the index variable will (correctly) fall out of HIR coverage.
        let elem_slot = self.hir_alloc_local(&element.text, &element_type);
        self.hir_open_block();
        self.analyze_body(
            body,
            ctx.parent_function,
            Some(&foreach_scope),
            true,
            diagnostics,
        )?;
        let body_hir = self.hir_close_block();
        self.hir_foreach(elem_slot, iter_hir, body_hir, label);
        Ok(())
    }
    pub(in crate::semantics::analyzer) fn analyze_while(
        &mut self,
        condition: &ExpressionNode<'a>,
        body: &[StatementNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let label = self.pending_loop_label.take();
        // An `is`-with-binding in the loop condition narrows a local for the body: the body only runs
        // when the condition holds, so the cast is sound and is re-established at the top of each
        // iteration. Covers a bare `while (x is T name)` and top-level `&&` chains, like `if`.
        let mut bindings: Vec<(&SyntaxToken, &Type, &ExpressionNode<'a>)> = Vec::new();
        Self::collect_is_bindings(condition, &mut bindings);
        let cond_type = self
            .analyze_expression(condition, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let cond_hir = self.hir_take();
        self.check_bool_condition("while", &cond_type, condition.position(), diagnostics);
        let body_scope = self.branch_scope(symbol_table);
        let ctx = super::super::AnalyzerContext {
            parent_function,
            symbol_table,
        };
        self.hir_open_block();
        self.declare_is_bindings(&bindings, &body_scope, &ctx, diagnostics)?;
        self.analyze_body(body, parent_function, Some(&body_scope), true, diagnostics)?;
        let body_hir = self.hir_close_block();
        self.hir_while(cond_hir, body_hir, label);
        Ok(())
    }
    pub(in crate::semantics::analyzer) fn analyze_do_while(
        &mut self,
        condition: &ExpressionNode<'a>,
        body: &[StatementNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let label = self.pending_loop_label.take();
        let cond_type = self
            .analyze_expression(condition, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let cond_hir = self.hir_take();
        self.check_bool_condition("do/while", &cond_type, condition.position(), diagnostics);
        self.hir_open_block();
        self.analyze_body(body, parent_function, Some(symbol_table), true, diagnostics)?;
        let body_hir = self.hir_close_block();
        self.hir_do_while(cond_hir, body_hir, label);
        Ok(())
    }
    pub(in crate::semantics::analyzer) fn analyze_for(
        &mut self,
        init: &Option<&'a StatementNode<'a>>,
        condition: &Option<ExpressionNode<'a>>,
        increment: &Option<&'a StatementNode<'a>>,
        body: &[StatementNode<'a>],
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let label = self.pending_loop_label.take();
        let for_scope = Rc::new(RefCell::new(SymbolTable::new(Some(
            ctx.symbol_table.clone(),
        ))));
        (*ctx.symbol_table)
            .borrow_mut()
            .add_child(for_scope.clone());

        self.hir_open_block();
        if let Some(init_stmt) = init {
            self.analyze_statement(
                init_stmt,
                ctx.parent_function,
                &for_scope,
                false,
                diagnostics,
            )?;
        }
        let init_hir = self.hir_close_block();

        let mut cond_hir = None;
        if let Some(cond_expr) = condition {
            let cond_type = self
                .analyze_expression(cond_expr, ctx.parent_function, &for_scope, diagnostics)
                .unwrap_or(Type::Unknown);
            cond_hir = self.hir_take();
            self.check_bool_condition("for", &cond_type, cond_expr.position(), diagnostics);
        }

        self.hir_open_block();
        if let Some(inc_stmt) = increment {
            self.analyze_statement(
                inc_stmt,
                ctx.parent_function,
                &for_scope,
                false,
                diagnostics,
            )?;
        }
        let step_hir = self.hir_close_block();

        self.hir_open_block();
        self.analyze_body(
            body,
            ctx.parent_function,
            Some(&for_scope),
            true,
            diagnostics,
        )?;
        let body_hir = self.hir_close_block();

        self.hir_for(init_hir, cond_hir, step_hir, body_hir, label);
        Ok(())
    }
}
