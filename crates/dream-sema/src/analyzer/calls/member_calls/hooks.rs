//! Indexer/enumerator protocol-hook resolution (`@get`/`@set`/`@iterator`/`@next`) shared by the
//! desugaring of `obj[i]`, `obj[i] = v`, and `for (let x in obj)`.

use super::super::super::*;
use dream_diagnostics::DiagnosticBag;
use crate::analyzer::declarations::protocol_hooks::{ProtocolHook, ProtocolRole};
use crate::function_table::FunctionTableInfo;
use dream_syntax::nodes::Type;

impl<'a> Analyzer<'a> {
    /// Resolves the registered `@…` protocol hook of `role` on `obj_type` and looks up its
    /// function-table entry. Shape was already validated at registration; this only fails when the
    /// type never declared that role.
    fn resolve_protocol_hook(
        &mut self,
        obj_type: &Type,
        role: ProtocolRole,
        diagnostics: &mut DiagnosticBag,
    ) -> Option<(ProtocolHook, FunctionTableInfo)> {
        let hook = self.protocol_hook(obj_type, role, diagnostics)?;
        let info = self
            .function_table
            .get_function(&hook.mangled_name)
            .ok()
            .or_else(|| {
                // Overloaded methods are stored under signature-mangled keys; fall back to the
                // first overload that matches the role's arity (excl. `this`).
                let keys = self.function_table.overloads.get(&hook.mangled_name)?;
                let expected = match role {
                    ProtocolRole::Get => 1,
                    ProtocolRole::Set => 2,
                    ProtocolRole::Iterator | ProtocolRole::Next => 0,
                };
                keys.iter().find_map(|k| {
                    let info = self.function_table.get_function(k).ok()?;
                    let declared = info.parameters.len().saturating_sub(1);
                    (declared == expected && !info.is_static && !info.is_async).then_some(info)
                })
            })?;
        Some((hook, info))
    }

    /// Resolves a protocol hook and, when absent, emits the site-specific diagnostic and returns
    /// `None`. Centralizes the Absent arm every desugaring site previously spelled out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolve_hook_or_diagnose(
        &mut self,
        obj_type: &Type,
        role: ProtocolRole,
        span: Option<dream_text::text_span::TextSpan>,
        clear_value: bool,
        diagnostics: &mut DiagnosticBag,
        absent: impl FnOnce() -> String,
    ) -> Option<(ProtocolHook, FunctionTableInfo)> {
        if let Some(resolved) = self.resolve_protocol_hook(obj_type, role, diagnostics) {
            return Some(resolved);
        }
        self.hir_fail();
        if clear_value {
            self.hir_none();
        }
        diagnostics.report_error(absent(), span);
        None
    }
}
