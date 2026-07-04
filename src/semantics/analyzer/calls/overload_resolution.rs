//! Analysis of call expressions: free-function and overload resolution, method calls, static /
//! namespaced calls (`Math.*` / `JSON.*` / async intrinsics / `derive` helpers), and constructors.

use super::super::*;
use crate::semantics::function_table::{FunctionTableInfo, OverloadResolution};

/// Outcome of looking up an indexer/enumerator "hook" method (`get`/`set`/`iterator`/`next`) on a
/// struct receiver, for the desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.

impl<'a> Analyzer<'a> {
    /// Resolves an overloaded base name against the concrete `arg_types`, returning the selected
    /// signature or a human-readable error (no match / ambiguous). Used by both free-function and
    /// method call analysis (methods prepend the receiver type as the implicit `this` argument).
    pub(crate) fn select_function_overload(
        &mut self,
        base: &str,
        arg_types: &[String],
    ) -> Result<FunctionTableInfo, String> {
        // Overload viability is a structural relation over interned ids (object widening, enum/int,
        // numeric, nullable): lower each spelling and defer to `types::overload_compatible` rather
        // than re-deriving the rules from strings.
        let type_ctx = &mut self.type_ctx;
        let compat = |param: &str, arg: &str| {
            let p = type_ctx.lower_str(param);
            let a = type_ctx.lower_str(arg);
            crate::types::overload_compatible(&type_ctx.interner, p, a)
        };
        match self.function_table.select_overload(base, arg_types, compat) {
            OverloadResolution::Unique(key) => match self.function_table.get_function(&key) {
                Ok(info) => Ok(info),
                Err(_) => Err(format!("Could not resolve function '{}'", key)),
            },
            OverloadResolution::None => Err(format!(
                "No overload of '{}' matches argument types ({})",
                base,
                arg_types.join(", ")
            )),
            OverloadResolution::Ambiguous(keys) => Err(format!(
                "Ambiguous call to '{}' with argument types ({}); candidates: {}",
                base,
                arg_types.join(", "),
                keys.join(", ")
            )),
        }
    }

    /// String-level assignability check for argument vs. parameter/field types, mirroring the
    /// rules in [`compare_data_type`] (which works on `Type`). An `expected` type accepts a `given`
    /// when they are identical, the target is `object`, they are enum/int compatible, or the target
    /// is nullable (`T?`) and the argument is `T`, `T?`, or the `null` literal (`void?`). Used by
    /// constructor-call checking, which only has the type names (not structured `Type`s) available.
    pub(crate) fn type_str_assignable(&mut self, expected: &str, given: &str) -> bool {
        // The poison type unifies with everything so an earlier error never cascades into a
        // spurious assignment/argument mismatch here. (Kept as an explicit name check because the
        // unknown spelling has no dedicated interned id.)
        if crate::syntax::nodes::types::is_unknown_type_name(expected)
            || crate::syntax::nodes::types::is_unknown_type_name(given)
        {
            return true;
        }
        // Directional assignability over interned types: `given` (value) must be assignable to
        // `expected` (target). Covers identity, `object` widening, enum/int, numeric widening, and
        // nullable/`null` handling via the structured rules.
        let e = self.type_ctx.lower_str(expected);
        let g = self.type_ctx.lower_str(given);
        if crate::types::assignable(&self.type_ctx.interner, e, g) {
            return true;
        }
        // Implicit upcast to an interface parameter: the argument's concrete class implements it.
        let iface = crate::syntax::nodes::types::strip_nullable(expected);
        if self.is_interface_name(iface) {
            let given_class = crate::syntax::nodes::types::strip_nullable(given);
            return self.implements_as_interface_ref(given_class, iface);
        }
        false
    }

    pub(crate) fn validate_arguments(
        &mut self,
        error_prefix: &str,
        expected: &[String],
        given: &[String],
        position: crate::text::text_span::TextSpan,
        diagnostics: &mut crate::diagnostics::DiagnosticBag,
    ) {
        for (i, given_type) in given.iter().enumerate() {
            if let Some(expected_type_str) = expected.get(i) {
                if !self.type_str_assignable(expected_type_str, given_type) {
                    diagnostics.report_error(
                        format!(
                            "{} expects parameter {} to be {}, got {}",
                            error_prefix,
                            i + 1,
                            expected_type_str,
                            given_type
                        ),
                        Some(position),
                    );
                }
            }
        }
    }
}
