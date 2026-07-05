//! Analysis of method-call expressions (`obj.method(args)`), split by dispatch shape:
//! - [`static_dispatch`]: `Type.method(args)` static/namespaced calls (union-variant construction,
//!   generic static-method monomorphization, `Math.*`/`JSON.*`/async intrinsics, plain statics).
//! - [`builtins`]: the `size()`/`char_at()`/`to_string()`/`hash_code()` object protocol available on
//!   every (or every primitive/array) receiver.
//! - [`instance_dispatch`]: ordinary and interface instance-method resolution once the receiver type
//!   is known and the static/builtin cases have been ruled out.
//! - [`hooks`]: indexer/enumerator "hook" method resolution (`get`/`set`/`iterator`/`next`) shared by
//!   the `obj[i]`, `obj[i] = v`, and `for..in` desugaring sites; `resolve_hook_or_diagnose` is the
//!   shared entry point those call, with the `HookResolution` outcome kept private to this module.

use super::super::*;
use crate::diagnostics::DiagnosticBag;
use crate::syntax::nodes::{ExpressionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;

mod builtins;
mod hooks;
mod instance_dispatch;
mod static_dispatch;

impl<'a> Analyzer<'a> {
    pub(crate) fn analyze_method_call(
        &mut self,
        obj: &ExpressionNode<'a>,
        method: &SyntaxToken,
        _generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, crate::semantics::errors::SemanticError> {
        if let ExpressionNode::Identifier(id) = obj {
            if let Some(t) =
                self.try_analyze_static_method(id, method, _generic_args, params, ctx, diagnostics)?
            {
                return Ok(t);
            }
        }

        let obj_type =
            self.analyze_expression(obj, ctx.parent_function, ctx.symbol_table, diagnostics)?;
        let obj_hir = self.hir_take();

        // The receiver was already poisoned by an earlier error; still type-check the arguments
        // (to surface their own mistakes) but stay quiet about the method itself and stay poison.
        if obj_type.is_unknown() {
            for param in params.iter() {
                let _ = self.analyze_expression(
                    param,
                    ctx.parent_function,
                    ctx.symbol_table,
                    diagnostics,
                );
            }
            self.hir_none();
            return Ok(Type::Unknown);
        }

        // A `js`-typed receiver binds dynamically: a declared `js` helper (e.g. `to_int`) dispatches
        // normally, any other name is a runtime JS method call. This must run before the builtin/
        // instance resolution below, which has no notion of dynamic members.
        if self.is_js_type(&obj_type) {
            return self.analyze_js_member_call(obj_hir, method, params, ctx, diagnostics);
        }

        // Builtin methods: `size()` lowers to `ArrayLen`; the rest (`to_string`/`char_at`/`hash_code`)
        // need runtime defs and stay on the legacy path (they clear HIR inside the helper). The
        // receiver is threaded in so `len` can wrap it; it is left intact when no builtin matches.
        let mut recv = obj_hir;
        if let Some(t) =
            self.analyze_builtin_method(&obj_type, method, params, ctx, &mut recv, diagnostics)?
        {
            return Ok(t);
        }

        self.analyze_instance_method(&obj_type, method, params, ctx, recv, diagnostics)
    }
}
