//! Call-argument analysis: typing, named-arg reorder, variadic packing, ref validation.

use super::super::*;
use dream_diagnostics::DiagnosticBag;
use dream_hir::{HExpr, HPlace};
use crate::errors::SemanticError;
use crate::symbol_table::SymbolTable;
use dream_syntax::nodes::{ExpressionNode, FunctionNode, Type};
use dream_text::text_span::TextSpan;
use std::cell::RefCell;
use std::rc::Rc;

type CallArgAnalysis = (Vec<String>, Vec<Option<HExpr>>, Vec<bool>);

impl<'a> Analyzer<'a> {
    /// Reorders a call's raw AST arguments into pure positional order when the source used named
    /// arguments (`f(a, name: value)`), resolving each `name` against `param_names` (the callee's
    /// declared parameter names) and filling any resulting gap from `defaults` (the callee's
    /// parallel default-value slice). Returns the input unchanged, with zero allocation, when no
    /// argument is a [`ExpressionNode::NamedArg`] — the overwhelmingly common case.
    ///
    /// Rules (reported as diagnostics, never a panic, on violation):
    /// - a name must match one of the nameable parameters, and each name may be supplied at most once;
    /// - when `is_variadic` is false, every positional argument must precede every named argument;
    /// - when `is_variadic` is true, the last parameter cannot be named; positionals after named
    ///   arguments are collected into the variadic `T[]` slot; fixed-parameter holes use defaults;
    /// - any fixed parameter left unfilled after positional+named assignment must have a default.
    ///
    /// The result may be shorter than `param_names` for non-variadic calls (trailing wholly-omitted
    /// optional parameters are left for trailing-default substitution), but never has an internal
    /// gap. Variadic calls always end with an `ArrayLiteral` for the variadic slot.
    pub(crate) fn normalize_named_arguments(
        &mut self,
        param_names: &[String],
        defaults: &[Option<Type>],
        raw_args: &[ExpressionNode<'a>],
        call_position: TextSpan,
        diagnostics: &mut DiagnosticBag,
        is_variadic: bool,
    ) -> Result<Vec<ExpressionNode<'a>>, SemanticError> {
        match Self::try_normalize_named_arguments(
            param_names,
            defaults,
            raw_args,
            call_position,
            is_variadic,
        ) {
            Ok(args) => Ok(args),
            Err((message, span)) => Err(report(diagnostics, message, span)),
        }
    }

    /// Dry-run counterpart of [`Self::normalize_named_arguments`]: returns the normalized argument
    /// list or `(message, span)` without touching diagnostics — used to filter overload candidates
    /// by named-argument binding before committing to one signature.
    /// Dry-run counterpart of [`Self::normalize_named_arguments`]: returns the normalized argument
    /// list or `(message, span)` without touching diagnostics — used to filter overload candidates
    /// by named-argument binding before committing to one signature.
    pub(crate) fn try_normalize_named_arguments(
        param_names: &[String],
        defaults: &[Option<Type>],
        raw_args: &[ExpressionNode<'a>],
        call_position: TextSpan,
        is_variadic: bool,
    ) -> Result<Vec<ExpressionNode<'a>>, (String, Option<TextSpan>)> {
        if !raw_args
            .iter()
            .any(|a| matches!(a, ExpressionNode::NamedArg(..)))
        {
            return Ok(raw_args.to_vec());
        }
        let fixed_len = if is_variadic {
            param_names.len().saturating_sub(1)
        } else {
            param_names.len()
        };
        let nameable = &param_names[..fixed_len];
        let mut slots: Vec<Option<ExpressionNode<'a>>> = vec![None; fixed_len];
        let mut variadic_tail: Vec<ExpressionNode<'a>> = Vec::new();
        let mut seen_named = false;
        let mut next_positional = 0usize;
        for arg in raw_args.iter() {
            match arg {
                ExpressionNode::NamedArg(name_tok, value) => {
                    if is_variadic
                        && param_names
                            .last()
                            .is_some_and(|n| n == &name_tok.text)
                    {
                        return Err((
                            format!(
                                "variadic parameter '{}' cannot be passed by name",
                                name_tok.text
                            ),
                            Some(name_tok.position),
                        ));
                    }
                    seen_named = true;
                    let Some(idx) = nameable.iter().position(|p| p == &name_tok.text) else {
                        return Err((
                            format!("no parameter named '{}'", name_tok.text),
                            Some(name_tok.position),
                        ));
                    };
                    if slots[idx].is_some() {
                        return Err((
                            format!("argument '{}' was already supplied", name_tok.text),
                            Some(name_tok.position),
                        ));
                    }
                    slots[idx] = Some((**value).clone());
                }
                other => {
                    if seen_named {
                        if is_variadic {
                            variadic_tail.push(other.clone());
                            continue;
                        }
                        return Err((
                            "a positional argument cannot follow a named argument".to_string(),
                            other.position().or(Some(call_position)),
                        ));
                    }
                    if next_positional < fixed_len {
                        if slots[next_positional].is_some() {
                            return Err((
                                format!(
                                    "argument '{}' was already supplied",
                                    nameable
                                        .get(next_positional)
                                        .cloned()
                                        .unwrap_or_else(|| format!("#{}", next_positional + 1))
                                ),
                                other.position().or(Some(call_position)),
                            ));
                        }
                        slots[next_positional] = Some(other.clone());
                        next_positional += 1;
                    } else if is_variadic {
                        variadic_tail.push(other.clone());
                    } else {
                        // Extra positional — leave for arity checking later by preserving as-is
                        // beyond fixed slots (non-variadic normalize historically sized slots to
                        // max(param_names, raw_args); keep trailing extras for the arity diagnostic).
                        slots.push(Some(other.clone()));
                    }
                }
            }
        }
        if is_variadic {
            for (i, slot) in slots.iter_mut().enumerate() {
                if slot.is_none() {
                    match defaults.get(i).and_then(|d| d.clone()) {
                        Some(default_lit) => *slot = Some(ExpressionNode::Literal(default_lit)),
                        None => {
                            let pname = nameable
                                .get(i)
                                .cloned()
                                .unwrap_or_else(|| format!("#{}", i + 1));
                            return Err((
                                format!("missing required argument '{}'", pname),
                                Some(call_position),
                            ));
                        }
                    }
                }
            }
            let mut result: Vec<ExpressionNode<'a>> =
                slots.into_iter().map(|s| s.unwrap()).collect();
            result.push(ExpressionNode::ArrayLiteral(variadic_tail));
            return Ok(result);
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
                        return Err((
                            format!("missing required argument '{}'", pname),
                            Some(call_position),
                        ));
                    }
                }
            }
        }
        Ok(slots.into_iter().map(|s| s.unwrap()).collect())
    }

    /// After overload selection, packs trailing analyzed arguments into a single array for a
    /// variadic parameter. No-ops when the args are already packed (`len == parameters.len()`).
    /// After overload selection, packs trailing analyzed arguments into a single array for a
    /// variadic parameter. No-ops when the args are already packed (`len == parameters.len()`).
    pub(crate) fn pack_variadic_analyzed_args(
        &mut self,
        sig: &crate::function_table::FunctionTableInfo,
        params_types: &mut Vec<String>,
        arg_hirs: &mut Vec<Option<HExpr>>,
        arg_is_ref: &mut Vec<bool>,
        skip: usize,
    ) {
        if !sig.is_variadic {
            return;
        }
        let total_user = sig.parameters.len().saturating_sub(skip);
        let fixed_user = total_user.saturating_sub(1);
        if params_types.len() == total_user {
            return;
        }
        if params_types.len() < fixed_user {
            return;
        }
        let array_ty_name = sig
            .parameters
            .get(skip + fixed_user)
            .cloned()
            .unwrap_or_default();
        let array_ty = Self::type_from_name(&array_ty_name);
        let tail_hirs: Vec<Option<HExpr>> = arg_hirs.drain(fixed_user..).collect();
        params_types.truncate(fixed_user);
        arg_is_ref.truncate(fixed_user);
        self.hir_set_array_lit(tail_hirs, &array_ty);
        arg_hirs.push(self.hir_take());
        params_types.push(array_ty.get_type());
        arg_is_ref.push(false);
    }

    /// Tries each overload's parameter names for a named-argument call and returns one shared
    /// normalized argument list when every successful candidate agrees. `user_param_offset` skips
    /// the implicit `this` for methods/constructors (`1`) or nothing for free functions (`0`).
    /// Tries each overload's parameter names for a named-argument call and returns one shared
    /// normalized argument list when every successful candidate agrees. `user_param_offset` skips
    /// the implicit `this` for methods/constructors (`1`) or nothing for free functions (`0`).
    pub(crate) fn normalize_named_for_overloads(
        &self,
        base: &str,
        raw_args: &[ExpressionNode<'a>],
        call_position: TextSpan,
        user_param_offset: usize,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Vec<ExpressionNode<'a>>, SemanticError> {
        let keys: Vec<String> = if let Some(keys) = self.function_table.overloads.get(base) {
            keys.clone()
        } else if self.function_table.functions.contains_key(base) {
            vec![base.to_string()]
        } else {
            return Err(report(
                diagnostics,
                format!("named arguments are not supported for '{}'", base),
                Some(call_position),
            ));
        };
        let mut successes: Vec<Vec<ExpressionNode<'a>>> = Vec::new();
        let mut last_err: Option<(String, Option<TextSpan>)> = None;
        for key in &keys {
            let Ok(info) = self.function_table.get_function(key) else {
                continue;
            };
            let param_names: Vec<String> = info
                .param_names
                .iter()
                .skip(user_param_offset)
                .cloned()
                .collect();
            let defaults: Vec<Option<Type>> =
                info.defaults.iter().skip(user_param_offset).cloned().collect();
            match Self::try_normalize_named_arguments(
                &param_names,
                &defaults,
                raw_args,
                call_position,
                info.is_variadic,
            ) {
                Ok(norm) => successes.push(norm),
                Err(e) => last_err = Some(e),
            }
        }
        if successes.is_empty() {
            let (message, span) = last_err.unwrap_or_else(|| {
                (
                    format!(
                        "no overload of '{}' accepts these named arguments",
                        base
                    ),
                    Some(call_position),
                )
            });
            return Err(report(diagnostics, message, span));
        }
        let first = successes[0].clone();
        for other in successes.iter().skip(1) {
            if other.len() != first.len() {
                return Err(report(
                    diagnostics,
                    format!(
                        "Ambiguous named-argument call to '{}': overloads disagree on argument layout",
                        base
                    ),
                    Some(call_position),
                ));
            }
        }
        Ok(first)
    }

    /// Resolves a `ref` call argument's inner place (`f(ref x)` / `f(ref obj.field)` /
    /// `f(ref arr[i])`) to its declared type and the HIR for the shared box pointer backing it.
    /// Locals/parameters reuse `CaptureCell`/`RefBox` slots; fields and array elements are copy-in/
    /// copy-out through a fresh temporary `RefBox` (writeback flushed after the call statement).
    /// Resolves a `ref` call argument's inner place (`f(ref x)` / `f(ref obj.field)` /
    /// `f(ref arr[i])`) to its declared type and the HIR for the shared box pointer backing it.
    /// Locals/parameters reuse `CaptureCell`/`RefBox` slots; fields and array elements are copy-in/
    /// copy-out through a fresh temporary `RefBox` (writeback flushed after the call statement).
    pub(crate) fn analyze_ref_argument(
        &mut self,
        inner: &'a ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Option<(Type, Option<HExpr>)> {
        match inner {
            ExpressionNode::Identifier(tok) => {
                let ty = match (**symbol_table).borrow().get_symbol(tok) {
                    Ok(t) => t,
                    Err(e) => {
                        diagnostics.report_error(e.to_string(), Some(tok.position));
                        self.hir_none();
                        return None;
                    }
                };
                let hir = self.hir_read_cell_ref(&tok.text);
                if hir.is_none() {
                    self.hir_fail();
                }
                Some((ty, hir))
            }
            ExpressionNode::MemberAccess(obj, member) => {
                let obj_ty = self
                    .analyze_expression(obj, parent_function, symbol_table, diagnostics)
                    .ok()?;
                let obj_hir = self.hir_take()?;
                let struct_name = match Self::resolve_struct_parts(&obj_ty) {
                    Some((base, _)) => base,
                    None => obj_ty.get_type(),
                };
                let Some(field_idx) = self.struct_field_index(&struct_name, &member.text) else {
                    diagnostics.report_error(
                        format!("'{}' has no field '{}'", struct_name, member.text),
                        Some(member.position),
                    );
                    self.hir_none();
                    return None;
                };
                let field_ty = self
                    .struct_table
                    .get_struct(&struct_name)
                    .and_then(|s| s.fields.get(&member.text).map(|f| f.type_.clone()))
                    .unwrap_or(Type::Unknown);
                self.hir_set_field(Some(obj_hir.clone()), field_idx, &field_ty);
                let value = self.hir_take()?;
                self.hir_box_ref_place(
                    HPlace::Field {
                        obj: Box::new(obj_hir),
                        field: field_idx,
                    },
                    &field_ty,
                    value,
                )
            }
            ExpressionNode::IndexAccess(array, index) => {
                let array_ty = self
                    .analyze_expression(array, parent_function, symbol_table, diagnostics)
                    .ok()?;
                let array_hir = self.hir_take()?;
                let _index_ty = self
                    .analyze_expression(index, parent_function, symbol_table, diagnostics)
                    .ok()?;
                let index_hir = self.hir_take()?;
                let elem_ty = match &array_ty {
                    Type::Array(inner) => (**inner).clone(),
                    _ => {
                        diagnostics.report_error(
                            format!(
                                "cannot take a 'ref' to an element of type '{}'",
                                array_ty.get_type()
                            ),
                            array.position(),
                        );
                        self.hir_none();
                        return None;
                    }
                };
                self.hir_set_index(Some(array_hir.clone()), Some(index_hir.clone()), &elem_ty);
                let value = self.hir_take()?;
                self.hir_box_ref_place(
                    HPlace::Index {
                        array: Box::new(array_hir),
                        index: Box::new(index_hir),
                    },
                    &elem_ty,
                    value,
                )
            }
            ExpressionNode::Parenthesized(inner) => {
                self.analyze_ref_argument(inner, parent_function, symbol_table, diagnostics)
            }
            _ => {
                diagnostics.report_error(
                    "a 'ref' argument must be a local variable, parameter, field, or array element"
                        .to_string(),
                    inner.position(),
                );
                self.hir_none();
                None
            }
        }
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
    /// True while analyzing a function/method declared in the embedded stdlib prelude (its
    /// `current_file` starts with the `<std>/` marker `driver::prelude` gives every prelude file —
    /// see `STD_PACKAGES` in `src/stdlib/mod.rs`). The prelude is compiler-authored and reviewed
    /// like the compiler itself, so it may call `@unsafe` primitives internally (e.g. `List<T>.grow`
    /// using `Buffer.realloc`) without forcing its own public, ordinarily-safe API to become
    /// `@unsafe` too — mirroring how `unsafe` blocks inside `std`/`core` don't make `Vec::push`
    /// unsafe to call. User code gets no such exemption: only `current_function_is_unsafe` (an
    /// explicit `@unsafe` on *that* function) can satisfy the gate there.
    /// Analyzes each explicit call argument in source order, returning the argument type strings
    /// alongside the HIR emitted for each. Shared by the static, instance, and interface call paths,
    /// which all analyze their arguments identically before resolving the callee. (The free-function
    /// path publishes per-parameter expected types while analyzing and so keeps its own loop.)
    pub(crate) fn analyze_call_arguments(
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
    /// Like [`analyze_call_arguments`](Self::analyze_call_arguments), but when `expected_params` is
    /// known (an unambiguous, non-overloaded callee), publishes each parameter's declared type as
    /// `current_expected_type` while analyzing the matching argument — mirroring the free-function
    /// call path (see `analyze_function_call`) so untyped literals and lambdas without their own
    /// annotation (e.g. `nums.sort_by((a: int, b: int) => a - b)`) can infer from a known `fun(...)`
    /// parameter type. `None` (an overloaded callee, whose parameter types aren't known until the
    /// arguments themselves are typed) falls back to no expected-type context, same as before.
    pub(crate) fn analyze_call_arguments_expecting(
        &mut self,
        params: &[ExpressionNode<'a>],
        expected_params: Option<&[Type]>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(Vec<String>, Vec<Option<HExpr>>), SemanticError> {
        let (arg_types, arg_hirs, _is_ref) = self.analyze_call_arguments_expecting_ref(
            params,
            expected_params,
            parent_function,
            symbol_table,
            diagnostics,
        )?;
        Ok((arg_types, arg_hirs))
    }

    /// Like [`analyze_call_arguments_expecting`](Self::analyze_call_arguments_expecting), but also
    /// returns which argument slots were written as a `ref` argument (`f(ref x)`), so the caller
    /// can validate each one against the resolved callee's `is_ref` parameter flags (mismatches are
    /// the caller's diagnostic, not this helper's — the callee's signature isn't always known yet
    /// at the point arguments are analyzed, e.g. before overload resolution).
    /// Like [`analyze_call_arguments_expecting`](Self::analyze_call_arguments_expecting), but also
    /// returns which argument slots were written as a `ref` argument (`f(ref x)`), so the caller
    /// can validate each one against the resolved callee's `is_ref` parameter flags (mismatches are
    /// the caller's diagnostic, not this helper's — the callee's signature isn't always known yet
    /// at the point arguments are analyzed, e.g. before overload resolution).
    pub(crate) fn analyze_call_arguments_expecting_ref(
        &mut self,
        params: &[ExpressionNode<'a>],
        expected_params: Option<&[Type]>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<CallArgAnalysis, SemanticError> {
        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        let mut arg_is_ref = Vec::new();
        for (i, param) in params.iter().enumerate() {
            let saved_expected = self.current_expected_type.take();
            self.current_expected_type = expected_params.and_then(|ps| ps.get(i).cloned());
            if let ExpressionNode::RefArgument(inner) = param {
                arg_is_ref.push(true);
                self.current_expected_type = saved_expected;
                match self.analyze_ref_argument(inner, parent_function, symbol_table, diagnostics) {
                    Some((t, hir)) => {
                        arg_hirs.push(hir);
                        arg_types.push(t.get_type());
                    }
                    None => {
                        arg_hirs.push(None);
                        arg_types.push(Type::Unknown.get_type());
                    }
                }
                continue;
            }
            arg_is_ref.push(false);
            let t = self.analyze_expression(param, parent_function, symbol_table, diagnostics)?;
            self.current_expected_type = saved_expected;
            arg_hirs.push(self.hir_take());
            arg_types.push(t.get_type());
        }
        Ok((arg_types, arg_hirs, arg_is_ref))
    }

    /// Reports a diagnostic for every argument slot whose `ref`-ness (`given_is_ref`) disagrees
    /// with the resolved callee's declared parameter (`expected_is_ref`) — either a plain value was
    /// passed where `ref` is required, or `ref` was written for a parameter that isn't one.
    /// Reports a diagnostic for every argument slot whose `ref`-ness (`given_is_ref`) disagrees
    /// with the resolved callee's declared parameter (`expected_is_ref`) — either a plain value was
    /// passed where `ref` is required, or `ref` was written for a parameter that isn't one.
    pub(crate) fn validate_ref_arguments(
        &mut self,
        error_prefix: &str,
        expected_is_ref: &[bool],
        given_is_ref: &[bool],
        position: TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        for (i, &given) in given_is_ref.iter().enumerate() {
            let expected = expected_is_ref.get(i).copied().unwrap_or(false);
            if given && !expected {
                diagnostics.report_error(
                    format!(
                        "{} expects parameter {} to be passed by value, but 'ref' was given",
                        error_prefix,
                        i + 1
                    ),
                    Some(position),
                );
            } else if !given && expected {
                diagnostics.report_error(
                    format!(
                        "{} expects parameter {} to be passed with 'ref', but it was passed by value",
                        error_prefix,
                        i + 1
                    ),
                    Some(position),
                );
            }
        }
    }
}
