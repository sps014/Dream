//! Analysis of statements and control flow: loops, switches, declarations, assignments,
//! `if`/`else`, `return`, and the `break`/`continue` placement checks.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::hir::{HExpr, HStmt};
use crate::intrinsics;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::{mangle_generic, strip_nullable};
use crate::syntax::nodes::{ExpressionNode, FunctionNode, StatementNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::types::method_fn;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(super) fn analyze_break(
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
    pub(super) fn analyze_continue(
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
    pub(super) fn analyze_foreach(
        &mut self,
        statement: &StatementNode<'a>,
        ctx: &super::AnalyzerContext<'a, '_>,
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
    pub(super) fn analyze_case_switch(
        &mut self,
        subject: &ExpressionNode<'a>,
        cases: &Vec<(Vec<ExpressionNode<'a>>, &'a [StatementNode<'a>])>,
        default_body: &Option<&'a [StatementNode<'a>]>,
        ctx: &super::AnalyzerContext<'a, '_>,
        has_parent_while: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let subject_type = self
            .analyze_expression(subject, ctx.parent_function, ctx.symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let subject_hir = self.hir_take();
        let mut hir_arms: Vec<crate::hir::HArm> = Vec::new();
        // A multi-label case (`case 1, 2, 3:`) becomes one `HArm` per label, all sharing a clone of
        // the case body (each label is a distinct dispatch target hitting the same code).
        let mut hir_ok = true;
        let subject_name = subject_type.get_type();
        let subject_is_enum = self.enum_table.contains_key(&subject_name);
        if !matches!(subject_name.as_str(), "int" | "string" | "bool") && !subject_is_enum {
            diagnostics.report_error(
                format!(
                    "switch subject must be int, string, bool, or an enum, got {}",
                    subject_name
                ),
                subject.position(),
            );
        }

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (labels, body) in cases.iter() {
            let mut label_hirs: Vec<Option<HExpr>> = Vec::new();
            for label in labels.iter() {
                // Labels must be compile-time constants: a literal, or (for enum switches) an
                // enum member access like `Color.Red`.
                let is_enum_label = matches!(label, ExpressionNode::MemberAccess(_, _));
                if !matches!(label, ExpressionNode::Literal(_)) && !is_enum_label {
                    diagnostics.report_error(
                        "switch case labels must be constant literals or enum members".to_string(),
                        label.position(),
                    );
                }
                let label_type = self
                    .analyze_expression(label, ctx.parent_function, ctx.symbol_table, diagnostics)
                    .unwrap_or(Type::Unknown);
                label_hirs.push(self.hir_take());
                self.compare_data_type(&subject_type, &label_type, &empty_span(), diagnostics)?;

                let key = match label {
                    ExpressionNode::Literal(
                        Type::Integer(t)
                        | Type::Float(t)
                        | Type::Double(t)
                        | Type::String(t)
                        | Type::Boolean(t),
                    ) => Some(t.text.clone()),
                    ExpressionNode::Literal(_) => None,
                    ExpressionNode::MemberAccess(_, m) => Some(m.text.clone()),
                    _ => None,
                };
                if let Some(k) = key {
                    if !seen.insert(k.clone()) {
                        diagnostics.report_error(
                            format!("duplicate case label '{}' in switch statement", k),
                            label.position(),
                        );
                    }
                }
            }
            self.hir_open_block();
            self.analyze_body(
                body,
                ctx.parent_function,
                Some(ctx.symbol_table),
                has_parent_while,
                diagnostics,
            )?;
            let body_hir = self.hir_close_block();
            // One arm per label; all labels of a case share (a clone of) its body.
            for label_hir in label_hirs {
                match self.hir_const_arm(label_hir, body_hir.clone()) {
                    Some(arm) => hir_arms.push(arm),
                    None => hir_ok = false,
                }
            }
        }

        let default_hir = if let Some(db) = default_body {
            self.hir_open_block();
            self.analyze_body(
                db,
                ctx.parent_function,
                Some(ctx.symbol_table),
                has_parent_while,
                diagnostics,
            )?;
            self.hir_close_block()
        } else {
            Vec::new()
        };

        self.hir_switch(subject_hir, hir_arms, default_hir, hir_ok);
        Ok(())
    }
    pub(super) fn analyze_while(
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
        if !cond_type.is_unknown() && !cond_type.is_bool() {
            diagnostics.report_error(
                format!("while condition must be bool, got {}", cond_type.get_type()),
                condition.position(),
            );
        }
        let body_scope = self.branch_scope(symbol_table);
        let ctx = super::AnalyzerContext {
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
    pub(super) fn analyze_do_while(
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
        if !cond_type.is_unknown() && !cond_type.is_bool() {
            diagnostics.report_error(
                format!(
                    "do/while condition must be bool, got {}",
                    cond_type.get_type()
                ),
                condition.position(),
            );
        }
        self.hir_open_block();
        self.analyze_body(body, parent_function, Some(symbol_table), true, diagnostics)?;
        let body_hir = self.hir_close_block();
        self.hir_do_while(cond_hir, body_hir, label);
        Ok(())
    }
    pub(super) fn analyze_for(
        &mut self,
        init: &Option<&'a StatementNode<'a>>,
        condition: &Option<ExpressionNode<'a>>,
        increment: &Option<&'a StatementNode<'a>>,
        body: &[StatementNode<'a>],
        ctx: &super::AnalyzerContext<'a, '_>,
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
            if !cond_type.is_unknown() && !cond_type.is_bool() {
                diagnostics.report_error(
                    format!("for condition must be bool, got {}", cond_type.get_type()),
                    cond_expr.position(),
                );
            }
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
    ///return type is returned currently int and float supported
    /// Reports a clear diagnostic when a reserved word (a builtin name or primitive type name) is
    /// used where a user-chosen identifier is expected (`role` is e.g. "variable"/"function").
    pub(super) fn check_reserved_name(
        &self,
        token: &SyntaxToken,
        role: &str,
        diagnostics: &mut DiagnosticBag,
    ) {
        // bare callable, so it is a legal ordinary identifier.
        const RESERVED_TYPE_AND_LITERAL_NAMES: &[&str] = &[
            "int", "float", "double", "string", "bool", "char", "object", "void", "long", "uint",
            "ulong", "byte",
            // C#/.NET-style aliases for the primitives (see `canonical_type_name`).
            "String", "Int32", "Int64", "UInt32", "UInt64", "Byte", "Single", "Double", "Boolean",
            "Char", "Object", "Void", "true", "false", "null",
        ];
        // The builtin callables are reserved too; sourced from the intrinsic registry so this list
        // never drifts from the set of names the compiler special-cases.
        let is_reserved = RESERVED_TYPE_AND_LITERAL_NAMES.contains(&token.text.as_str())
            || intrinsics::is_object_builtin(&token.text);
        if is_reserved {
            diagnostics.report_error(
                format!(
                    "'{}' is a reserved word and cannot be used as a {} name",
                    token.text, role
                ),
                Some(token.position),
            );
        }
    }

    pub(super) fn analyze_declaration(
        &mut self,
        left: &SyntaxToken,
        type_annotation: &Option<Type>,
        right: &ExpressionNode<'a>,
        is_const: bool,
        ctx: &super::AnalyzerContext<'a, '_>,
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
    pub(super) fn analyze_assignment(
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

    pub(super) fn analyze_index_assignment(
        &mut self,
        arr: &'a ExpressionNode<'a>,
        index: &'a ExpressionNode<'a>,
        right: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let array_type = self
            .analyze_expression(arr, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let array_hir = self.hir_take();

        // A `js`-typed receiver: `obj[key] = v` sets a JS property/element dynamically.
        if self.is_js_type(&array_type) {
            let _key_type = self
                .analyze_expression(index, parent_function, symbol_table, diagnostics)
                .unwrap_or(Type::Unknown);
            let key_hir = self.hir_take();
            let _value_type = self
                .analyze_expression(right, parent_function, symbol_table, diagnostics)
                .unwrap_or(Type::Unknown);
            let value_hir = self.hir_take();
            self.desugar_js_index_set(array_hir, key_hir, value_hir, index.position(), diagnostics);
            return Ok(());
        }

        // Class index-assignment: `obj[i] = v` on a struct receiver desugars to `obj.set(i, v)`
        // when an eligible `set` exists. Arrays keep the built-in path.
        if !matches!(array_type, Type::Array(_) | Type::Unknown)
            && Self::resolve_struct_parts(&array_type).is_some()
        {
            // The synthesized call re-evaluates the receiver, so drop the base HIR taken above.
            let _ = array_hir;
            return self.analyze_index_set(
                arr,
                index,
                right,
                &array_type,
                parent_function,
                symbol_table,
                diagnostics,
            );
        }

        let inner_type = match array_type {
            Type::Array(inner) => *inner,
            _ => {
                self.hir_fail();
                diagnostics.report_error(
                    format!("Cannot index into non-array type {}", array_type.get_type()),
                    arr.position(),
                );
                return Ok(());
            }
        };

        let index_type = self
            .analyze_expression(index, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let index_hir = self.hir_take();
        if !index_type.is_unknown() && !index_type.is_int() {
            diagnostics.report_error(
                format!(
                    "Array index must be of type int, got {}",
                    index_type.get_type()
                ),
                index.position(),
            );
        }

        let right_type = self
            .analyze_expression(right, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let value_hir = self.hir_take();
        self.compare_data_type(&inner_type, &right_type, &empty_span(), diagnostics)?;

        self.hir_assign_index(array_hir, index_hir, value_hir);
        Ok(())
    }

    /// Desugars a class index-assignment `obj[index] = value` to `obj.set(index, value)` when
    /// `obj_type` exposes an eligible `set` (accessible instance, non-async method taking two
    /// arguments; its return value is discarded). A same-named `set` that is static/async/wrong
    /// arity is left as an ordinary method and this site reports why the value is not
    /// index-assignable.
    fn analyze_index_set(
        &mut self,
        arr: &'a ExpressionNode<'a>,
        index: &'a ExpressionNode<'a>,
        right: &ExpressionNode<'a>,
        obj_type: &Type,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        if self
            .resolve_hook_or_diagnose(
                obj_type,
                "set",
                2,
                arr.position(),
                false,
                diagnostics,
                |reason| {
                    format!(
                        "type '{}' is not index-assignable: {}",
                        obj_type.get_type(),
                        reason
                    )
                },
                || {
                    format!(
                        "type '{}' is not index-assignable (define 'public fun set(index, value)' to allow obj[index] = value)",
                        obj_type.get_type()
                    )
                },
            )
            .is_none()
        {
            return Ok(());
        }
        let set_tok = synthetic_token(TokenKind::IdentifierToken, "set");
        let call =
            ExpressionNode::MethodCall(arr, set_tok, None, vec![(*index).clone(), right.clone()]);
        let _ = self.analyze_expression(&call, parent_function, symbol_table, diagnostics)?;
        let call_hir = self.hir_take();
        self.hir_expr_stmt(call_hir);
        Ok(())
    }

    pub(super) fn analyze_member_assignment(
        &mut self,
        obj: &'a ExpressionNode<'a>,
        member: &SyntaxToken,
        right: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        // Static property setter `Type.prop = v`: when the receiver names a type (not a local) and a
        // static setter exists, desugar to a static call `Type.set$prop(v)` (mirrors the instance
        // setter desugar below, but the receiver is the type rather than a value).
        if let ExpressionNode::Identifier(id) = obj {
            let is_local = symbol_table.borrow().get_symbol(id).is_ok();
            if !is_local {
                let type_name = crate::syntax::nodes::types::canonical_type_name(&id.text)
                    .unwrap_or(id.text.as_str())
                    .to_string();
                let setter = method_fn(&type_name, &setter_member_name(&member.text));
                if self.function_table.get_function(&setter).is_ok() {
                    let set_tok = synthetic_token(
                        TokenKind::IdentifierToken,
                        &setter_member_name(&member.text),
                    );
                    let call = ExpressionNode::MethodCall(obj, set_tok, None, vec![right.clone()]);
                    let _ =
                        self.analyze_expression(&call, parent_function, symbol_table, diagnostics)?;
                    let call_hir = self.hir_take();
                    self.hir_expr_stmt(call_hir);
                    return Ok(());
                }
            }
        }

        let obj_type = self
            .analyze_expression(obj, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let obj_hir = self.hir_take();

        // A `js`-typed receiver: `obj.name = v` sets a JS property dynamically.
        if self.is_js_type(&obj_type) {
            let _value_type = self
                .analyze_expression(right, parent_function, symbol_table, diagnostics)
                .unwrap_or(Type::Unknown);
            let value_hir = self.hir_take();
            self.desugar_js_set(
                obj_hir,
                &member.text,
                value_hir,
                Some(member.position),
                diagnostics,
            );
            return Ok(());
        }

        let (base_name, generic_args) = match Self::resolve_struct_parts(&obj_type) {
            Some(parts) => parts,
            None => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "Cannot access member of non-class type {}",
                        obj_type.get_type()
                    ),
                    Some(member.position),
                );
                return Ok(());
            }
        };

        self.ensure_struct_instantiated(&base_name, &generic_args, &member.position, diagnostics);
        let struct_name = mangle_generic(&base_name, &generic_args);

        let field = match self.struct_table.get_struct(&struct_name) {
            Some(info) => info
                .fields
                .get(&member.text)
                .map(|i| (i.type_.clone(), i.is_public)),
            None => {
                self.hir_fail();
                diagnostics.report_error(
                    format!("Struct '{}' not found", struct_name),
                    Some(member.position),
                );
                return Ok(());
            }
        };

        let (field_type, field_is_public) = match field {
            Some(f) => f,
            None => {
                // Not a field: `obj.prop = v` may write a property setter, which desugars to a call
                // of the (internally named) setter method. The call carries its own privacy/type
                // check, and its (discarded) result becomes the assignment statement.
                let setter = method_fn(&struct_name, &setter_member_name(&member.text));
                if self.function_table.get_function(&setter).is_ok() {
                    let set_tok = synthetic_token(
                        TokenKind::IdentifierToken,
                        &setter_member_name(&member.text),
                    );
                    let call = ExpressionNode::MethodCall(obj, set_tok, None, vec![right.clone()]);
                    let _ =
                        self.analyze_expression(&call, parent_function, symbol_table, diagnostics)?;
                    let call_hir = self.hir_take();
                    self.hir_expr_stmt(call_hir);
                    return Ok(());
                }
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "Field '{}' not found in class '{}'",
                        member.text, struct_name
                    ),
                    Some(member.position),
                );
                return Ok(());
            }
        };

        // Private fields (the default) may only be written from within the declaring type's own
        // methods; `public` exposes them to outside code.
        if !field_is_public && !self.in_methods_of(parent_function, &base_name) {
            diagnostics.report_error(
                format!("'{}' is private to '{}'", member.text, base_name),
                Some(member.position),
            );
        }

        // Publish the field's declared type so the right-hand side can infer against it
        // (e.g. `this.items = []` resolves the empty literal to the field's element type).
        let saved_expected = self.current_expected_type.take();
        self.current_expected_type = Some(field_type.clone());
        let right_type = self
            .analyze_expression(right, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let value_hir = self.hir_take();
        self.current_expected_type = saved_expected;
        self.compare_data_type(&field_type, &right_type, &member.position, diagnostics)?;

        match self.struct_field_index(&struct_name, &member.text) {
            Some(index) => self.hir_assign_field(obj_hir, index, value_hir),
            None => self.hir_fail(),
        }
        Ok(())
    }
    pub(super) fn analyze_if_else(
        &mut self,
        statement: &StatementNode<'a>,
        ctx: &super::AnalyzerContext<'a, '_>,
        has_parent_while: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let StatementNode::IfElse(condition, if_body, else_if, else_body) = statement else {
            unreachable!()
        };
        // Live branches of the chain in source order, each `(condition HIR, body)`. An `is` condition
        // on a concrete (non-`object`) operand folds to a compile-time constant: a `false` branch is
        // dead (skipped entirely, so its body — valid only under other instantiations — is never
        // type-checked), and a `true` branch is unconditionally taken, becoming the terminal `else`
        // and ending the chain. Regular conditions are analyzed normally and keep their HIR.
        let mut arms: Vec<(HExpr, Vec<HStmt>)> = Vec::new();
        let mut terminal: Vec<HStmt> = Vec::new();
        let mut terminated = false;

        // Every branch of the chain (primary, then each `else if`) as `(condition, position, body)`.
        let branches = std::iter::once((condition, condition.position(), if_body))
            .chain(else_if.iter().map(|i| (&i.0, i.0.position(), &i.1)));

        for (cond_expr, cond_pos, body) in branches {
            // An `is`-with-binding condition declares a narrowed local `name: T` scoped to the taken
            // branch only. This covers a bare `if (x is T name)` and every `is`-binding reachable
            // through a top-level `&&` chain (`if (a && x is T name)`), each of which is guaranteed to
            // hold in the then-branch. Both the compile-time fold and the runtime path introduce them
            // into that branch's scope.
            let mut bindings: Vec<(&SyntaxToken, &Type, &ExpressionNode<'a>)> = Vec::new();
            Self::collect_is_bindings(cond_expr, &mut bindings);

            // `is` fold: an operand with a concrete (non-`object`, non-interface) static type resolves
            // at compile time, so a branch is either taken unconditionally or is dead. An `object` or
            // interface operand needs a runtime tag check, so it falls through to the general
            // (runtime-`IsType`) path below.
            if let ExpressionNode::IsExpression(left, right_type, _) = cond_expr {
                let left_t = self
                    .analyze_expression(left, ctx.parent_function, ctx.symbol_table, diagnostics)
                    .unwrap_or(Type::Unknown);
                let left_name = strip_nullable(&left_t.get_type()).to_string();
                let runtime =
                    left_t.is_object() || left_t.is_unknown() || self.is_interface_name(&left_name);
                if !runtime {
                    if left_t.get_type() == right_type.get_type() {
                        let branch_scope = self.branch_scope(ctx.symbol_table);
                        self.hir_open_block();
                        self.declare_is_bindings(&bindings, &branch_scope, ctx, diagnostics)?;
                        self.analyze_body(
                            body,
                            ctx.parent_function,
                            Some(&branch_scope),
                            has_parent_while,
                            diagnostics,
                        )?;
                        terminal = self.hir_close_block();
                        terminated = true;
                        break;
                    } else {
                        // Dead branch: skip it entirely (do not analyze its body).
                        continue;
                    }
                }
            }

            let cond_type = self
                .analyze_expression(
                    cond_expr,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                )
                .unwrap_or(Type::Unknown);
            let cond_hir = self.hir_take();
            if !cond_type.is_unknown() && !cond_type.is_bool() {
                diagnostics.report_error(
                    format!("if condition must be bool, got {}", cond_type.get_type()),
                    cond_pos,
                );
            }
            let branch_scope = self.branch_scope(ctx.symbol_table);
            self.hir_open_block();
            self.declare_is_bindings(&bindings, &branch_scope, ctx, diagnostics)?;
            self.analyze_body(
                body,
                ctx.parent_function,
                Some(&branch_scope),
                has_parent_while,
                diagnostics,
            )?;
            let body_hir = self.hir_close_block();
            match cond_hir {
                Some(cond_hir) => arms.push((cond_hir, body_hir)),
                None => self.hir_fail(),
            }
        }

        if !terminated {
            if let Some(body) = else_body {
                self.hir_open_block();
                self.analyze_body(
                    body,
                    ctx.parent_function,
                    Some(ctx.symbol_table),
                    has_parent_while,
                    diagnostics,
                )?;
                terminal = self.hir_close_block();
            }
        }

        // Fold the live arms (innermost last) into a single nested `if`/`else` and emit it.
        let mut chain = terminal;
        for (cond, body) in arms.into_iter().rev() {
            chain = vec![crate::hir::HStmt::If {
                cond,
                then_branch: body,
                else_branch: chain,
            }];
        }
        for stmt in chain {
            self.hir_push_stmt(stmt);
        }
        Ok(())
    }

    /// A fresh child symbol scope of `parent`, used for a single `if`/`else if` branch so an
    /// `is`-with-binding local lives only inside that branch and never leaks to `else`/the enclosing
    /// scope. Outer names stay visible through the parent link.
    fn branch_scope(&self, parent: &Rc<RefCell<SymbolTable>>) -> Rc<RefCell<SymbolTable>> {
        let scope = Rc::new(RefCell::new(SymbolTable::new(Some(Rc::clone(parent)))));
        (*parent).borrow_mut().add_child(scope.clone());
        scope
    }

    /// Collects the `is`-with-binding conditions that are guaranteed true whenever `cond` is true:
    /// a bare `x is T name`, and every such test reachable through a top-level `&&` chain (descending
    /// through parentheses). `||`, negation, and other operators are *not* descended into, because a
    /// binding under them is not guaranteed to hold when the whole condition is true. The collected
    /// bindings are declared into the taken branch (or loop body) by [`Self::declare_is_bindings`].
    fn collect_is_bindings<'e>(
        cond: &'e ExpressionNode<'a>,
        out: &mut Vec<(&'e SyntaxToken, &'e Type, &'e ExpressionNode<'a>)>,
    ) {
        match cond {
            ExpressionNode::IsExpression(left, right_type, Some(name)) => {
                out.push((name, right_type, left));
            }
            ExpressionNode::Parenthesized(inner) => Self::collect_is_bindings(inner, out),
            ExpressionNode::Binary(left, op, right)
                if op.kind == TokenKind::AmpersandAmpersandToken =>
            {
                Self::collect_is_bindings(left, out);
                Self::collect_is_bindings(right, out);
            }
            _ => {}
        }
    }

    /// Declares each collected `is`-binding into `branch_scope` (see [`Self::declare_is_binding`]).
    /// A no-op when there are no bindings. Must be called inside the target block's open HIR block,
    /// before its body, so the narrowed-local declarations lead the block.
    fn declare_is_bindings(
        &mut self,
        bindings: &[(&SyntaxToken, &Type, &ExpressionNode<'a>)],
        branch_scope: &Rc<RefCell<SymbolTable>>,
        ctx: &super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        for &(name, target_ty, operand) in bindings {
            self.declare_is_binding(name, target_ty, operand, branch_scope, ctx, diagnostics)?;
        }
        Ok(())
    }

    /// Introduces an `is`-with-binding local into a branch: it declares `name: T` (added to
    /// `branch_scope` for type-checking) initialized by an implicit `(T)operand` cast. Reusing the
    /// cast path means reference/interface targets alias the same pointer (identity) while value-type
    /// targets (`int`, `bool`, …) unbox the operand — exactly the narrowing `is` implies. Must be
    /// called inside the branch's open HIR block, before its body.
    fn declare_is_binding(
        &mut self,
        name: &SyntaxToken,
        target_ty: &Type,
        operand: &ExpressionNode<'a>,
        branch_scope: &Rc<RefCell<SymbolTable>>,
        ctx: &super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        self.check_reserved_name(name, "variable", diagnostics);
        // Model the narrowed initializer as `(target_ty)operand`, reusing all cast validation +
        // codegen (reference identity vs. primitive unbox).
        let _ = self.analyze_cast(
            target_ty,
            operand,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;
        let init = self.hir_take();
        self.hir_declare_local(&name.text, target_ty, init);
        if let Err(e) = (*branch_scope)
            .borrow_mut()
            .add_symbol(name.text.clone(), target_ty.clone())
        {
            diagnostics.report_error(e.to_string(), Some(name.position));
        }
        Ok(())
    }
    pub(super) fn analyze_return(
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
