//! Indexer/enumerator "hook" method resolution (`get`/`set`/`iterator`/`next`) shared by the
//! desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.

use super::super::super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::function_table::FunctionTableInfo;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::Type;
use crate::types::method_fn;

/// Outcome of looking up an indexer/enumerator "hook" method (`get`/`set`/`iterator`/`next`) on a
/// struct receiver, for the desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.
// One variant carries a resolved method descriptor while others are unit-like; the value is
// short-lived and never stored en masse, so the size spread does not warrant boxing.
#[allow(clippy::large_enum_variant)]
enum HookResolution {
    /// The receiver is not a struct, or it has no method with that name: the sugar is unavailable.
    Absent,
    /// A method with that name exists but cannot serve as a hook; carries a human-readable reason
    /// (it is static, async, or has the wrong number of parameters).
    Ineligible(String),
    /// A usable hook: an accessible instance, non-async method with the requested declared arity.
    Eligible(FunctionTableInfo),
}

impl<'a> Analyzer<'a> {
    /// Resolves a hook method named `method_name` (with declared arity `declared_arity`, i.e.
    /// excluding the implicit `this`) on struct receiver `obj_type`, ensuring the receiver's generic
    /// instance is registered first. Return-type shape checks (non-void for `get`, `Option<T>` for
    /// `next`, etc.) are left to the caller. An overloaded hook resolves to the first overload that
    /// matches the requested arity. A same-named method that is `static`, `async`, or of the wrong
    /// arity yields `Ineligible` (so `obj[i]`/`for..in` never silently hijack an ordinary method),
    /// while a call like `obj.get(i)` keeps resolving through the normal method path.
    fn resolve_hook_method(
        &mut self,
        obj_type: &Type,
        method_name: &str,
        declared_arity: usize,
        diagnostics: &mut DiagnosticBag,
    ) -> HookResolution {
        let (base_name, generic_args) = match Self::resolve_struct_parts(obj_type) {
            Some(parts) => {
                self.ensure_type_instantiated(&parts.0, &parts.1, &empty_span(), diagnostics);
                parts
            }
            // `string` is a built-in reference type carrying `extend string` methods (registered
            // under the `string` type name), so its `get`/`iterator` hooks resolve exactly like a
            // class's — no instantiation needed since `string` is not generic.
            None if matches!(obj_type, Type::String(_)) => ("string".to_string(), Vec::new()),
            None => return HookResolution::Absent,
        };
        let mono_name = mangle_generic(&base_name, &generic_args);
        let mangled = method_fn(&mono_name, method_name);

        let candidates: Vec<FunctionTableInfo> = if self.function_table.is_overloaded(&mangled) {
            self.function_table
                .overloads
                .get(&mangled)
                .map(|keys| {
                    keys.iter()
                        .filter_map(|k| self.function_table.get_function(k).ok())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            match self.function_table.get_function(&mangled) {
                Ok(info) => vec![info],
                Err(_) => return HookResolution::Absent,
            }
        };

        // Prefer an eligible candidate; otherwise remember why the first candidate was unusable.
        let mut ineligible_reason: Option<String> = None;
        for info in candidates {
            if info.is_static {
                ineligible_reason.get_or_insert_with(|| {
                    format!("'{}' must be a non-static instance method", method_name)
                });
                continue;
            }
            if info.is_async {
                ineligible_reason
                    .get_or_insert_with(|| format!("'{}' cannot be async", method_name));
                continue;
            }
            // Instance methods carry an implicit `this` at parameter index 0.
            let declared = info.parameters.len().saturating_sub(1);
            if declared != declared_arity {
                ineligible_reason.get_or_insert_with(|| {
                    format!(
                        "'{}' must take {} argument(s), but takes {}",
                        method_name, declared_arity, declared
                    )
                });
                continue;
            }
            return HookResolution::Eligible(info);
        }
        match ineligible_reason {
            Some(reason) => HookResolution::Ineligible(reason),
            None => HookResolution::Absent,
        }
    }

    /// Resolves a hook (see [`Analyzer::resolve_hook_method`]) and, when it is unusable, emits the
    /// site-specific diagnostic for you and returns `None`: marks HIR failed (also clearing the
    /// pending value when `clear_value`), then reports `ineligible(reason)` or `absent()` at `span`.
    /// Centralizes the identical Ineligible/Absent arms every desugaring site (`obj[i]`, `obj[i] = v`,
    /// `for..in`) previously spelled out; callers keep only their `Eligible` logic.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolve_hook_or_diagnose(
        &mut self,
        obj_type: &Type,
        method_name: &str,
        declared_arity: usize,
        span: Option<crate::text::text_span::TextSpan>,
        clear_value: bool,
        diagnostics: &mut DiagnosticBag,
        ineligible: impl FnOnce(&str) -> String,
        absent: impl FnOnce() -> String,
    ) -> Option<FunctionTableInfo> {
        let message =
            match self.resolve_hook_method(obj_type, method_name, declared_arity, diagnostics) {
                HookResolution::Eligible(info) => return Some(info),
                HookResolution::Ineligible(reason) => ineligible(&reason),
                HookResolution::Absent => absent(),
            };
        self.hir_fail();
        if clear_value {
            self.hir_none();
        }
        diagnostics.report_error(message, span);
        None
    }
}
