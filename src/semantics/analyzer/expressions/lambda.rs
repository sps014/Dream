//! Arrow-lambda literals (`(params) => expr` / `(params) => { stmts }`). A lambda is lowered to an
//! ordinary synthesized top-level function (`__lambda_<n>`); non-capturing, it behaves exactly like
//! any other free function. A *capturing* one (Milestone B — see `capture_scan`'s module doc
//! comment for how a capture is found, including transitively through further-nested lambdas)
//! additionally receives its captured names through the `fun(...)` value's environment word: one
//! capture as a direct `__Cell<T>` pointer, two or more as an `object[]` array of them (see
//! `hir_set_capturing_func_value`/`hir_set_multi_capturing_func_value`).
//!
//! Because Dream requires a function's return type up front (to type-check its body and to build
//! its function-table signature) and the synthesized function's body cannot be analyzed until a
//! later deferred pass (see `pending_lambdas` / `analyze_pending_instantiations`), the return type
//! must be knowable *immediately*, at the lambda literal's own use site. It is taken from the
//! expected `fun(...)` type of the surrounding context (a `let` annotation, a typed call argument,
//! etc.) — the same context used by `instantiate_generic_function_value` for generic function
//! values. A lambda used where no such context exists is rejected with a diagnostic asking for one.

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
        // The return type — and any parameter left untyped (`(a, b) => ...`, parsed with a
        // `Type::Unknown` placeholder; see `parse_lambda_parameters`) — must come from the expected
        // `fun(...)` context at this use site: the return type can't be inferred by analyzing the
        // body eagerly (see the module doc comment), and an untyped parameter has nothing else to
        // infer from in v1 (no unification/generalization over the body).
        let expected = self
            .current_expected_type
            .as_ref()
            .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings));
        let (exp_params, ret_type) = match expected {
            Some(Type::Function(exp_params, exp_ret))
                if exp_params.len() == lambda.parameters.len() =>
            {
                (exp_params, *exp_ret)
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

        // Resolve each parameter: an explicit annotation is checked against the expected type;
        // an omitted one (`Type::Unknown`) is filled in from it.
        let mut parameters: Vec<ParameterNode> = Vec::with_capacity(lambda.parameters.len());
        for (param, exp) in lambda.parameters.iter().zip(exp_params.iter()) {
            match &param.type_ {
                Type::Unknown => {
                    parameters.push(ParameterNode::with_default(
                        param.name.clone(),
                        exp.clone(),
                        param.default.clone(),
                    ));
                }
                declared => {
                    if declared.get_type() != exp.get_type() {
                        diagnostics.report_error(
                            format!(
                                "lambda parameter type mismatch: expected {}, got {}",
                                exp.get_type(),
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

        let name = format!("__lambda_{}", self.lambda_counter);
        self.lambda_counter += 1;

        // Free-variable capture (Milestone B). A free name is only a real capture if it resolves as
        // a local (parameter/`let`, of this lambda's own immediately-enclosing scope — transitively
        // including a name a lambda nested further inside `lambda` itself needs, see
        // `capture_scan`'s module doc comment) via `symbol_table`; a module-level global needs no
        // capture at all (the lifted function already resolves globals directly, same as any other
        // free function). Sorted for a deterministic env-array slot order.
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
            generic_parameters: None,
            generic_constraints: Vec::new(),
            return_type: Some(ret_type.clone()),
            parameters,
            body,
            is_public: false,
            is_extern: false,
            is_static: false,
            is_async: false,
            file_path: parent_function.file_path.clone(),
            accessor: None,
            is_default_impl: false,
        };
        let func_ref: &'a FunctionNode<'a> = self.arena.alloc(func_node);

        let info = FunctionTableInfo::from(func_ref);
        // Synthesized names are always fresh (a monotonically increasing counter), so this cannot
        // collide with a user function or an earlier lambda.
        let _ = self.function_table.add_function(name.clone(), info);
        self.type_ctx.register(DefKind::Function, &name, vec![]);
        self.pending_lambdas.insert(name.clone(), func_ref);

        let func_ty = Type::Function(param_types, Box::new(ret_type.clone()));
        match captures.len() {
            0 => self.hir_set_func_value(&name, &func_ty, &ret_type),
            1 => {
                let (cap_name, cap_ty) = captures[0].clone();
                // Registered *before* `analyze_pending_instantiations` reaches this lambda's own
                // deferred body analysis, which consults it twice: to add `cap_name: cap_ty` into
                // the lifted function's own symbol table (`add_function_param_table`) and to emit
                // its env-unboxing prologue (`hir_begin_function`).
                self.closure_captures
                    .insert(name.clone(), vec![(cap_name.clone(), cap_ty)]);
                match self.hir_read_cell_ref(&cap_name) {
                    Some(cell) => {
                        self.hir_retain_env(cell.clone());
                        self.hir_set_capturing_func_value(&name, cell, &func_ty, &ret_type);
                    }
                    None => self.hir_none(),
                }
            }
            _ => {
                self.closure_captures.insert(name.clone(), captures.clone());
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
                    self.hir_set_multi_capturing_func_value(&name, cells, &func_ty, &ret_type);
                } else {
                    self.hir_none();
                }
            }
        }
        Ok(func_ty)
    }
}
