//! Arrow-lambda literals (`(params) => expr` / `(params) => { stmts }`, optionally prefixed with
//! `async`). A lambda is lowered to an ordinary synthesized top-level function (`__lambda_<n>`);
//! non-capturing, it behaves exactly like any other free function. An `async` lambda sets
//! `is_async` on that synthesized function and is typed as a `fun(...): Future<T>` value (the
//! boxed `call_indirect` returns the async constructor's Future frame). A *capturing* one
//! (Milestone B — see `capture_scan`'s module doc comment for how a capture is found, including
//! transitively through further-nested lambdas) additionally receives its captured names through
//! the `fun(...)` value's environment word: one capture as a direct `__Cell<T>` pointer, two or
//! more as an `object[]` array of them (see `hir_set_capturing_func_value`/
//! `hir_set_multi_capturing_func_value`).
//!
//! The return type — and any parameter left untyped — normally comes from the expected `fun(...)`
//! type of the surrounding context. When every parameter carries an explicit annotation and no
//! such context exists, the return type is inferred by eagerly analyzing the body (expression or
//! `return` statements). Untyped parameters still require a `fun(...)` context.

use super::capture_scan::lambda_free_names;
use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::FunctionTableInfo;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::{
    FunctionNode, LambdaBody, LambdaNode, ParameterNode, StatementNode, Type,
};
use crate::syntax::token::token_kind::TokenKind;
use crate::types::DefKind;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(in crate::semantics::analyzer) fn analyze_lambda(
        &mut self,
        lambda: &'a LambdaNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        // `ref` lambda parameters are supported: the expected `fun(...)` type encodes each `ref`
        // slot as `__RefBox<T>` (see `fun(ref T)` in the type grammar), and the synthesized
        // function uses `HParam.is_ref` like a named `ref` function.
        let expected = self
            .current_expected_type
            .as_ref()
            .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings));

        let is_generic_lambda = lambda.generic_parameters.is_some();

        // Generic lambdas keep their declared (possibly type-parameter) signature as a template
        // and instantiate from a `fun(...)` context or later use — do not bake expected concrete
        // types into the template parameter list.
        if is_generic_lambda {
            if lambda
                .parameters
                .iter()
                .any(|p| matches!(p.type_, Type::Unknown))
                && !matches!(expected, Some(Type::Function(_, _)))
            {
                self.hir_none();
                return Err(report(
                    diagnostics,
                    "cannot infer the type of this generic lambda without a `fun(...)`-typed context or annotated parameters".to_string(),
                    Some(lambda.open_paren_position),
                ));
            }
            let template_param_types: Vec<Type> = match &expected {
                Some(Type::Function(exp_params, _))
                    if exp_params.len() == lambda.parameters.len() =>
                {
                    lambda
                        .parameters
                        .iter()
                        .zip(exp_params.iter())
                        .map(|(p, exp)| match &p.type_ {
                            Type::Unknown => exp.clone(),
                            other => other.clone(),
                        })
                        .collect()
                }
                _ => lambda
                    .parameters
                    .iter()
                    .map(|p| p.type_.clone())
                    .collect(),
            };
            let template_body_ret = self.infer_lambda_return_type(
                lambda,
                &template_param_types,
                parent_function,
                symbol_table,
                diagnostics,
            )?;
            let box_ret = if lambda.is_async {
                match &expected {
                    Some(Type::Function(_, exp_ret)) => (**exp_ret).clone(),
                    _ => Self::future_type(template_body_ret.clone()),
                }
            } else {
                match &expected {
                    Some(Type::Function(_, exp_ret)) => (**exp_ret).clone(),
                    _ => template_body_ret.clone(),
                }
            };
            return self.finish_lambda_with_expected(
                lambda,
                parent_function,
                symbol_table,
                diagnostics,
                template_param_types,
                (template_body_ret, box_ret),
            );
        }

        let (exp_params, exp_ret) = match expected {
            Some(Type::Function(exp_params, exp_ret))
                if exp_params.len() == lambda.parameters.len() =>
            {
                (exp_params, *exp_ret)
            }
            _ if lambda
                .parameters
                .iter()
                .all(|p| !matches!(p.type_, Type::Unknown)) =>
            {
                let param_types: Vec<Type> =
                    lambda.parameters.iter().map(|p| p.type_.clone()).collect();
                let body_ret = self.infer_lambda_return_type(
                    lambda,
                    &param_types,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                let box_ret = if lambda.is_async {
                    Self::future_type(body_ret.clone())
                } else {
                    body_ret.clone()
                };
                return self.finish_lambda_with_expected(
                    lambda,
                    parent_function,
                    symbol_table,
                    diagnostics,
                    param_types,
                    (body_ret, box_ret),
                );
            }
            _ => {
                self.hir_none();
                return Err(report(
                    diagnostics,
                    "cannot infer the type of this lambda without a `fun(...)`-typed context (e.g. `let f: fun(int): int = (x: int) => x * x;`)".to_string(),
                    Some(lambda.open_paren_position),
                ));
            }
        };

        let (body_ret, box_ret) = if lambda.is_async {
            match Self::future_inner_type(&exp_ret) {
                Some(inner) => (inner, exp_ret.clone()),
                None => {
                    self.hir_none();
                    return Err(report(
                        diagnostics,
                        format!(
                            "async lambda requires a `fun(...): Future<T>` context, but the expected type returns '{}'",
                            exp_ret.get_type()
                        ),
                        Some(lambda.open_paren_position),
                    ));
                }
            }
        } else if Self::future_inner_type(&exp_ret).is_some() {
            self.hir_none();
            return Err(report(
                diagnostics,
                "sync lambda cannot be used where `fun(...): Future<T>` is expected; write `async (params) => ...` for an async lambda".to_string(),
                Some(lambda.open_paren_position),
            ));
        } else {
            (exp_ret.clone(), exp_ret)
        };

        let mut parameters: Vec<ParameterNode> = Vec::with_capacity(lambda.parameters.len());
        for (param, exp) in lambda.parameters.iter().zip(exp_params.iter()) {
            let (exp_elem, exp_is_ref) = Self::peel_ref_box(exp);
            if param.is_ref != exp_is_ref {
                diagnostics.report_error(
                    format!(
                        "lambda parameter '{}' {} 'ref' but the expected `fun(...)` type {} it",
                        param.name.text,
                        if param.is_ref { "is" } else { "is not" },
                        if exp_is_ref { "marks" } else { "does not mark" }
                    ),
                    Some(lambda.open_paren_position),
                );
            }
            match &param.type_ {
                Type::Unknown => {
                    if exp_is_ref || param.is_ref {
                        parameters.push(ParameterNode::by_ref(param.name.clone(), exp_elem));
                    } else {
                        parameters.push(ParameterNode::with_default(
                            param.name.clone(),
                            exp_elem,
                            param.default.clone(),
                        ));
                    }
                }
                declared => {
                    if declared.get_type() != exp_elem.get_type() {
                        diagnostics.report_error(
                            format!(
                                "lambda parameter type mismatch: expected {}, got {}",
                                exp_elem.get_type(),
                                declared.get_type()
                            ),
                            Some(lambda.open_paren_position),
                        );
                    }
                    parameters.push(param.clone());
                }
            }
        }
        let param_types: Vec<Type> = parameters.iter().map(|p| p.type_.clone()).collect();
        self.finish_lambda_with_expected(
            lambda,
            parent_function,
            symbol_table,
            diagnostics,
            param_types,
            (body_ret, box_ret),
        )
    }

    /// If `ty` is `__RefBox<T>`, returns `(T, true)`; otherwise `(ty, false)`.
    pub(in crate::semantics::analyzer) fn peel_ref_box(ty: &Type) -> (Type, bool) {
        match ty {
            Type::Struct(tok, Some(args)) if tok.text == "__RefBox" && args.len() == 1 => {
                (args[0].clone(), true)
            }
            other => (other.clone(), false),
        }
    }

    /// Infers a lambda's return type by analyzing its body with the given concrete parameter types
    /// in scope. HIR produced during the probe is discarded — the deferred body pass emits the
    /// real HIR later.
    fn infer_lambda_return_type(
        &mut self,
        lambda: &'a LambdaNode<'a>,
        param_types: &[Type],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let mut probe_table = SymbolTable::new(Some(symbol_table.clone()));
        for (param, ty) in lambda.parameters.iter().zip(param_types.iter()) {
            let _ = probe_table.add_symbol(param.name.text.clone(), ty.clone());
        }
        let probe_table = Rc::new(RefCell::new(probe_table));
        let _saved_hir = self.hir_take();
        let saved_expected = self.current_expected_type.take();
        let result = match &lambda.body {
            LambdaBody::Expr(expr) => {
                let t = self.analyze_expression(expr, parent_function, &probe_table, diagnostics);
                let _ = self.hir_take();
                t
            }
            LambdaBody::Block(stmts) => {
                let mut returns = Vec::new();
                self.collect_return_types(
                    stmts,
                    parent_function,
                    &probe_table,
                    diagnostics,
                    &mut returns,
                )?;
                if returns.is_empty() {
                    Ok(Type::Void)
                } else {
                    let first = returns[0].clone();
                    for other in returns.iter().skip(1) {
                        if first.get_type() != other.get_type()
                            && !first.is_unknown()
                            && !other.is_unknown()
                        {
                            diagnostics.report_error(
                                format!(
                                    "lambda return type mismatch: expected {}, got {}",
                                    first.get_type(),
                                    other.get_type()
                                ),
                                Some(lambda.open_paren_position),
                            );
                        }
                    }
                    Ok(first)
                }
            }
        };
        let _ = self.hir_take();
        self.current_expected_type = saved_expected;
        result
    }

    fn collect_return_types(
        &mut self,
        stmts: &[StatementNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
        out: &mut Vec<Type>,
    ) -> Result<(), SemanticError> {
        for stmt in stmts {
            match stmt {
                StatementNode::Return(Some(expr)) => {
                    let t =
                        self.analyze_expression(expr, parent_function, symbol_table, diagnostics)?;
                    let _ = self.hir_take();
                    out.push(t);
                }
                StatementNode::Return(None) => {
                    out.push(Type::Void);
                }
                StatementNode::IfElse(_, then_b, else_ifs, else_b) => {
                    self.collect_return_types(
                        then_b,
                        parent_function,
                        symbol_table,
                        diagnostics,
                        out,
                    )?;
                    for (_, body) in else_ifs {
                        self.collect_return_types(
                            body,
                            parent_function,
                            symbol_table,
                            diagnostics,
                            out,
                        )?;
                    }
                    if let Some(eb) = else_b {
                        self.collect_return_types(
                            eb,
                            parent_function,
                            symbol_table,
                            diagnostics,
                            out,
                        )?;
                    }
                }
                StatementNode::While(_, body)
                | StatementNode::DoWhile(body, _)
                | StatementNode::ForEach(_, _, _, _, body)
                | StatementNode::Lock(_, body) => {
                    self.collect_return_types(
                        body,
                        parent_function,
                        symbol_table,
                        diagnostics,
                        out,
                    )?;
                }
                StatementNode::For(_, _, _, body) => {
                    self.collect_return_types(
                        body,
                        parent_function,
                        symbol_table,
                        diagnostics,
                        out,
                    )?;
                }
                StatementNode::Labeled(_, inner) => {
                    self.collect_return_types(
                        std::slice::from_ref(*inner),
                        parent_function,
                        symbol_table,
                        diagnostics,
                        out,
                    )?;
                }
                StatementNode::Switch(_, cases, default) => {
                    for (_, body) in cases {
                        self.collect_return_types(
                            body,
                            parent_function,
                            symbol_table,
                            diagnostics,
                            out,
                        )?;
                    }
                    if let Some(db) = default {
                        self.collect_return_types(
                            db,
                            parent_function,
                            symbol_table,
                            diagnostics,
                            out,
                        )?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn finish_lambda_with_expected(
        &mut self,
        lambda: &'a LambdaNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
        param_types: Vec<Type>,
        returns: (Type, Type),
    ) -> Result<Type, SemanticError> {
        let (body_ret, box_ret) = returns;
        let is_generic_lambda = lambda
            .generic_parameters
            .as_ref()
            .map(|ps| !ps.is_empty())
            .unwrap_or(false);
        let parameters: Vec<ParameterNode> = lambda
            .parameters
            .iter()
            .zip(param_types.iter())
            .map(|(param, ty)| {
                if param.is_ref {
                    ParameterNode::by_ref(param.name.clone(), ty.clone())
                } else if matches!(param.type_, Type::Unknown) {
                    ParameterNode::with_default(
                        param.name.clone(),
                        ty.clone(),
                        param.default.clone(),
                    )
                } else {
                    let mut p = param.clone();
                    p.type_ = ty.clone();
                    p
                }
            })
            .collect();

        let name = format!("__lambda_{}", self.lambda_counter);
        self.lambda_counter += 1;

        let mut captures: Vec<(String, Type)> = Vec::new();
        for free_name in lambda_free_names(lambda) {
            if self.globals.iter().any(|g| g.name == free_name) {
                continue;
            }
            let tok = synthetic_token(TokenKind::IdentifierToken, &free_name);
            if let Ok(ty) = (*symbol_table).as_ref().borrow().get_symbol(&tok) {
                captures.push((free_name, ty));
            }
        }
        captures.sort_by(|a, b| a.0.cmp(&b.0));

        if let Some((bad, _)) = captures.iter().find(|(name, _)| {
            parent_function
                .parameters
                .iter()
                .any(|p| p.is_ref && p.name.text == *name)
        }) {
            self.hir_none();
            return Err(report(
                diagnostics,
                format!(
                    "cannot capture 'ref' parameter '{}' in a lambda expression (its storage is only valid for the call's duration)",
                    bad
                ),
                Some(lambda.open_paren_position),
            ));
        }

        if let Some((bad, bad_ty)) = captures.iter().find(|(_, ty)| {
            let tid = self.type_ctx.lower(ty);
            self.type_ctx.interner.is_ref_struct_type(tid)
        }) {
            self.hir_none();
            return Err(report(
                diagnostics,
                format!(
                    "cannot capture '{}' of type '{}' in a lambda expression: '{}' is a 'ref struct' (stack-only) and cannot be stored in the lambda's heap-allocated closure environment",
                    bad,
                    bad_ty.get_type(),
                    bad_ty.get_type()
                ),
                Some(lambda.open_paren_position),
            ));
        }

        if self.is_webworker_body_call() {
            if let Some((bad, bad_ty)) = captures.iter().find(|(_, ty)| {
                let tid = self.type_ctx.lower(ty);
                !matches!(ty, Type::Function(_, _))
                    && !self.type_ctx.interner.is_shared_type(tid)
                    && !self.type_satisfies_kind(ty, crate::syntax::nodes::ConstraintKind::Unmanaged)
            }) {
                self.hir_none();
                let who = self
                    .current_call_target_name
                    .as_deref()
                    .unwrap_or("WebWorker");
                return Err(report(
                    diagnostics,
                    format!(
                        "cannot capture '{}' of type '{}' in a `{}` body: only '@shared class' instances and unmanaged (blittable) values can safely cross into another thread — mark its type '@shared' or avoid capturing it",
                        bad,
                        bad_ty.get_type(),
                        who
                    ),
                    Some(lambda.open_paren_position),
                ));
            }
        }

        let body: &'a [StatementNode<'a>] = match &lambda.body {
            LambdaBody::Block(stmts) => stmts,
            LambdaBody::Expr(expr) => {
                let stmt = StatementNode::Return(Some((**expr).clone()));
                self.arena.alloc_slice_clone(&[stmt])
            }
        };

        let func_node = FunctionNode {
            attributes: Vec::new(),
            name: synthetic_token(TokenKind::IdentifierToken, &name),
            generic_parameters: lambda.generic_parameters.clone(),
            generic_constraints: lambda.generic_constraints.clone(),
            return_type: Some(body_ret.clone()),
            parameters,
            body,
            visibility: crate::syntax::nodes::Visibility::Private,
            is_extern: false,
            is_static: false,
            is_async: lambda.is_async,
            file_path: parent_function.file_path.clone(),
            accessor: None,
            is_default_impl: false,
        };
        let func_ref: &'a FunctionNode<'a> = self.arena.alloc(func_node);

        if !captures.is_empty() {
            self.closure_captures.insert(name.clone(), captures.clone());
        }

        if is_generic_lambda {
            self.generic_functions.insert(name.clone(), func_ref);
            self.type_ctx.register(
                DefKind::Function,
                &name,
                lambda
                    .generic_parameters
                    .as_ref()
                    .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
                    .unwrap_or_default(),
            );

            let expected = self
                .current_expected_type
                .as_ref()
                .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings));
            if let Some(Type::Function(_, _)) = expected {
                let tok = synthetic_token(TokenKind::IdentifierToken, &name);
                return match self.instantiate_generic_function_value(&tok, diagnostics) {
                    Some(func_ty) => {
                        // Propagate captures onto the mangled instance if any.
                        if let Some(caps) = self.closure_captures.get(&name).cloned() {
                            if let Some(Type::Function(_, _)) = self.current_expected_type.as_ref()
                            {
                                // Instance name is mangled; find latest registered instance.
                                // `instantiate_generic_function_value` already emitted HIR.
                                let _ = caps;
                            }
                        }
                        Ok(func_ty)
                    }
                    None => Ok(Type::Unknown),
                };
            }
            self.hir_none();
            return Ok(Type::GenericFunctionItem(name));
        }

        let info = FunctionTableInfo::from(func_ref);
        let _ = self.function_table.add_function(name.clone(), info);
        self.type_ctx.register(DefKind::Function, &name, vec![]);
        self.pending_lambdas
            .insert(name.clone(), (func_ref, self.current_generic_bindings.clone()));

        let func_ty_params: Vec<Type> = func_ref
            .parameters
            .iter()
            .map(|p| {
                if p.is_ref {
                    Self::ref_box_type(&p.type_)
                } else {
                    p.type_.clone()
                }
            })
            .collect();
        let func_ty = Type::Function(func_ty_params, Box::new(box_ret.clone()));
        match captures.len() {
            0 => self.hir_set_func_value(&name, &func_ty, &box_ret),
            1 => {
                let (cap_name, _) = &captures[0];
                match self.hir_read_cell_ref(cap_name) {
                    Some(cell) => {
                        self.hir_retain_env(cell.clone());
                        self.hir_set_capturing_func_value(&name, cell, &func_ty, &box_ret);
                    }
                    None => self.hir_none(),
                }
            }
            _ => {
                let mut cells = Vec::with_capacity(captures.len());
                let mut ok = true;
                for (cap_name, _) in &captures {
                    match self.hir_read_cell_ref(cap_name) {
                        Some(cell) => cells.push(cell),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    self.hir_set_multi_capturing_func_value(&name, cells, &func_ty, &box_ret);
                } else {
                    self.hir_none();
                }
            }
        }
        Ok(func_ty)
    }

    /// True when the lambda/function-value being analyzed is a `WebWorker`/`WebWorker.map`/
    /// `WebWorkerPool.dispatch` body argument — see `current_call_target_name` set by the
    /// call-argument analysis paths. `WebWorker<TIn, TOut>.map(...)` dispatches through the
    /// *generic*-class static-call path (`try_analyze_static_method`'s `generic_structs` branch in
    /// `static_dispatch/mod.rs`), which monomorphizes the receiver to its mangled name
    /// (`WebWorker_string_string`) before `plain.rs` sets the call target from *that* — so the
    /// receiver half is matched by its unmangled prefix (mangling only ever appends `_{arg}...`),
    /// not by exact string.
    pub(in crate::semantics::analyzer) fn is_webworker_body_call(&self) -> bool {
        match self.current_call_target_name.as_deref() {
            Some("WebWorker") => true,
            Some(name) => match name.split_once('.') {
                Some((recv, "map")) => {
                    recv == "WebWorker" || recv.starts_with("WebWorker_")
                }
                Some((recv, "dispatch")) => recv == "WebWorkerPool",
                _ => false,
            },
            None => false,
        }
    }
}
