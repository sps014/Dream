//! Call-expression analysis, grouped by call shape:
//! - [`free_functions`]: free-function calls and overload selection entry points.
//! - [`member_calls`]: instance/static/namespaced method calls, plus the indexer/enumerator "hook"
//!   resolution (`get`/`set`/`iterator`/`next`) used to desugar `obj[i]`, `obj[i] = v`, and
//!   `for..in`. `resolve_hook_or_diagnose` there is the shared entry point those desugaring sites
//!   call; the `HookResolution` outcome is an implementation detail kept private to that module.
//! - [`overload_resolution`]: scoring/ranking of candidate overloads.
//! - [`constructor`]: constructor-call analysis.

pub(crate) mod constructor;
pub(crate) mod free_functions;
pub(crate) mod member_calls;
pub(crate) mod overload_resolution;

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::hir::HExpr;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use dream_text::text_span::TextSpan;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// The structured (never string-mangled) parameter types of `sig`, suitable for publishing as
    /// `current_expected_type` while analyzing a non-overloaded callee's arguments. Prefers
    /// `sig.parameter_types` (populated by `FunctionTableInfo::from`, so a generic-struct-typed
    /// parameter like `List<int>` keeps its `Struct(name, Some(args))` shape); falls back to
    /// reconstructing from the mangled `sig.parameters` strings for synthesized/stdlib entries
    /// that never populate `parameter_types` (only ever primitives there, so the round-trip is
    /// lossless).
    pub(super) fn expected_param_types(sig: &crate::semantics::function_table::FunctionTableInfo) -> Vec<Type> {
        if sig.parameter_types.len() == sig.parameters.len() {
            sig.parameter_types.clone()
        } else {
            sig.parameters.iter().map(|p| Self::type_from_name(p)).collect()
        }
    }

    /// Reorders a call's raw AST arguments into pure positional order when the source used named
    /// arguments (`f(a, name: value)`), resolving each `name` against `param_names` (the callee's
    /// declared parameter names) and filling any resulting gap from `defaults` (the callee's
    /// parallel default-value slice). Returns the input unchanged, with zero allocation, when no
    /// argument is a [`ExpressionNode::NamedArg`] — the overwhelmingly common case.
    ///
    /// Rules (reported as diagnostics, never a panic, on violation):
    /// - every positional argument must precede every named argument;
    /// - a name must match one of `param_names`, and each name may be supplied at most once;
    /// - any parameter left unfilled after positional+named assignment must have a default in
    ///   `defaults` at that index, which is spliced in as a literal (identical to what an omitted
    ///   trailing default already produces via [`Self::substitute_default_args`]).
    ///
    /// The result may be shorter than `param_names` (any further trailing, wholly-omitted optional
    /// parameters are left for the existing trailing-default substitution to fill), but never has
    /// an internal gap — every hole up to the highest supplied/named index is resolved here.
    pub(crate) fn normalize_named_arguments(
        &mut self,
        param_names: &[String],
        defaults: &[Option<Type>],
        raw_args: &[ExpressionNode<'a>],
        call_position: TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Vec<ExpressionNode<'a>>, SemanticError> {
        if !raw_args
            .iter()
            .any(|a| matches!(a, ExpressionNode::NamedArg(..)))
        {
            return Ok(raw_args.to_vec());
        }
        let mut slots: Vec<Option<ExpressionNode<'a>>> =
            vec![None; param_names.len().max(raw_args.len())];
        let mut seen_named = false;
        for (i, arg) in raw_args.iter().enumerate() {
            match arg {
                ExpressionNode::NamedArg(name_tok, value) => {
                    seen_named = true;
                    let Some(idx) = param_names.iter().position(|p| p == &name_tok.text) else {
                        return Err(report(
                            diagnostics,
                            format!("no parameter named '{}'", name_tok.text),
                            Some(name_tok.position),
                        ));
                    };
                    if slots[idx].is_some() {
                        return Err(report(
                            diagnostics,
                            format!("argument '{}' was already supplied", name_tok.text),
                            Some(name_tok.position),
                        ));
                    }
                    slots[idx] = Some((**value).clone());
                }
                other => {
                    if seen_named {
                        return Err(report(
                            diagnostics,
                            "a positional argument cannot follow a named argument".to_string(),
                            other.position().or(Some(call_position)),
                        ));
                    }
                    slots[i] = Some(other.clone());
                }
            }
        }
        while matches!(slots.last(), Some(None)) {
            slots.pop();
        }
        for (i, slot) in slots.iter_mut().enumerate() {
            if slot.is_none() {
                match defaults.get(i).and_then(|d| d.clone()) {
                    Some(default_lit) => *slot = Some(ExpressionNode::Literal(default_lit)),
                    None => {
                        let pname = param_names
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| format!("#{}", i + 1));
                        return Err(report(
                            diagnostics,
                            format!("missing required argument '{}'", pname),
                            Some(call_position),
                        ));
                    }
                }
            }
        }
        Ok(slots.into_iter().map(|s| s.unwrap()).collect())
    }

    /// Collects a variadic call's trailing arguments into a single array literal, so the rest of
    /// argument analysis sees an ordinary positional list whose last slot is the `T[]` value the
    /// variadic parameter expects — reusing the existing array-literal analysis path (including
    /// its empty-literal element-type inference from `current_expected_type`, which the caller's
    /// per-index `expected_params` loop already publishes for this slot) rather than adding a
    /// dedicated variadic HIR/MIR shape. `total_params` is the callee's full declared parameter
    /// count (the variadic slot included). Returns the input unchanged when there are too few
    /// arguments to reach the variadic slot at all, leaving the existing arity check to report the
    /// shortfall with its usual message.
    pub(crate) fn collect_variadic_args(
        &self,
        total_params: usize,
        raw_args: &[ExpressionNode<'a>],
    ) -> Vec<ExpressionNode<'a>> {
        if total_params == 0 || raw_args.len() < total_params - 1 {
            return raw_args.to_vec();
        }
        let mut result: Vec<ExpressionNode<'a>> = raw_args[..total_params - 1].to_vec();
        let tail: Vec<ExpressionNode<'a>> = raw_args[total_params - 1..].to_vec();
        result.push(ExpressionNode::ArrayLiteral(tail));
        result
    }
    /// Analyzes each explicit call argument in source order, returning the argument type strings
    /// alongside the HIR emitted for each. Shared by the static, instance, and interface call paths,
    /// which all analyze their arguments identically before resolving the callee. (The free-function
    /// path publishes per-parameter expected types while analyzing and so keeps its own loop.)
    pub(super) fn analyze_call_arguments(
        &mut self,
        params: &[ExpressionNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(Vec<String>, Vec<Option<HExpr>>), SemanticError> {
        self.analyze_call_arguments_expecting(
            params,
            None,
            parent_function,
            symbol_table,
            diagnostics,
        )
    }

    /// Like [`analyze_call_arguments`](Self::analyze_call_arguments), but when `expected_params` is
    /// known (an unambiguous, non-overloaded callee), publishes each parameter's declared type as
    /// `current_expected_type` while analyzing the matching argument — mirroring the free-function
    /// call path (see `analyze_function_call`) so untyped literals and lambdas without their own
    /// annotation (e.g. `nums.sort_by((a: int, b: int) => a - b)`) can infer from a known `fun(...)`
    /// parameter type. `None` (an overloaded callee, whose parameter types aren't known until the
    /// arguments themselves are typed) falls back to no expected-type context, same as before.
    pub(super) fn analyze_call_arguments_expecting(
        &mut self,
        params: &[ExpressionNode<'a>],
        expected_params: Option<&[Type]>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(Vec<String>, Vec<Option<HExpr>>), SemanticError> {
        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        for (i, param) in params.iter().enumerate() {
            let saved_expected = self.current_expected_type.take();
            self.current_expected_type = expected_params.and_then(|ps| ps.get(i).cloned());
            let t = self.analyze_expression(param, parent_function, symbol_table, diagnostics)?;
            self.current_expected_type = saved_expected;
            arg_hirs.push(self.hir_take());
            arg_types.push(t.get_type());
        }
        Ok((arg_types, arg_hirs))
    }
}
