//! `Type.method(args)` static/namespaced call analysis, split by concern:
//! - this module: the `try_analyze_static_method` dispatch (variant construction, generic-class
//!   static methods, and routing to the generic-static / plain paths).
//! - [`intrinsics`]: on-the-fly monomorphization of generic static methods, including the
//!   `System.print`, `Buffer.alloc`, `Bytes.of`/`to`, `Promise.*`, and `JSON.*` intrinsics.
//! - [`plain`]: plain (non-generic) static-method resolution (`analyze_static_call`).

use super::super::super::*;
use crate::syntax::nodes::types::{canonical_type_name, mangle_generic};
use crate::syntax::nodes::ExpressionNode;
use crate::types::method_fn;

mod intrinsics;
mod plain;

impl<'a> Analyzer<'a> {
    /// Handles `Type.method(args)` static dispatch when the receiver `id` names a type rather than
    /// a local: discriminated-union variant construction, on-the-fly monomorphization of generic
    /// static methods (including the `Buffer.alloc` and promise-combinator intrinsics), and plain
    /// static-method resolution. Returns `Ok(Some(type))` when handled, `Ok(None)` when `id` is a
    /// local or names no static member (so the caller falls through to instance dispatch).
    pub(crate) fn try_analyze_static_method(
        &mut self,
        id: &SyntaxToken,
        method: &SyntaxToken,
        generic_args: &Option<Vec<Type>>,
        params: &Vec<ExpressionNode<'a>>,
        ctx: &AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Option<Type>, SemanticError> {
        // The receiver names a type (not a local variable), so resolve `{type}_{method}` directly
        // with no implicit `this`.
        let is_local = (*ctx.symbol_table).as_ref().borrow().get_symbol(id).is_ok();
        if is_local {
            return Ok(None);
        }

        // `Enum.Variant(args)`: construct a discriminated-union value.
        if let Some(t) = self.analyze_variant_construction(
            &id.text,
            method,
            params,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )? {
            return Ok(Some(t));
        }

        let type_name = canonical_type_name(&id.text)
            .unwrap_or(id.text.as_str())
            .to_string();
        let base = method_fn(&type_name, &method.text);

        // File/module-level visibility (Axis 2): reaching a static member requires the type itself
        // to be visible. A non-public class/struct is only referenceable from its declaring file.
        if let Some(info) = self.struct_table.get_struct(&type_name) {
            if !self.visible_across_files(
                &info.file_path,
                info.is_public,
                ctx.parent_function.file_path.as_ref(),
            ) {
                let decl_file = info.file_path.clone();
                self.report_not_public("Type", &type_name, &decl_file, id.position, diagnostics);
            }
        } else if let Some(template) = self.generic_structs.get(&type_name) {
            let (decl_file, is_public) = (template.file_path.clone(), template.is_public);
            if !self.visible_across_files(
                &decl_file,
                is_public,
                ctx.parent_function.file_path.as_ref(),
            ) {
                self.report_not_public("Type", &type_name, &decl_file, id.position, diagnostics);
            }
        } else {
            self.check_type_visible(
                &type_name,
                ctx.parent_function.file_path.as_ref(),
                id.position,
                diagnostics,
            );
        }

        // A static method on a *generic class* (`Cache<int>.make(...)`). The class's type arguments
        // ride on the call's generic-argument slot (attached by the parser). Monomorphize the class
        // so its concrete static methods (`Cache_int_make`, ...) are registered, then dispatch the
        // concrete method through the normal static-call path (which enforces class-level privacy).
        if self.generic_structs.contains_key(&type_name) {
            let args: Vec<Type> = match generic_args {
                Some(a) if !a.is_empty() => a
                    .iter()
                    .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings))
                    .collect(),
                _ => {
                    diagnostics.report_error(
                        format!(
                            "Generic class '{}' requires type arguments to call a static method, e.g. {}<int>.{}(...)",
                            type_name, type_name, method.text
                        ),
                        Some(id.position),
                    );
                    return Ok(Some(Type::Unknown));
                }
            };
            self.ensure_struct_instantiated(&type_name, &args, &id.position, diagnostics);
            let mangled_type = mangle_generic(&type_name, &args);
            let ret = self.analyze_static_call(
                &mangled_type,
                method,
                params,
                ctx.parent_function,
                ctx.symbol_table,
                diagnostics,
            )?;
            return Ok(Some(ret));
        }

        // Support generic static method calls by monomorphizing them on the fly.
        if let Some(&template) = self.generic_functions.get(&base) {
            let t = self.analyze_generic_static_method(
                template,
                &base,
                &type_name,
                method,
                generic_args,
                params,
                ctx,
                diagnostics,
            )?;
            return Ok(Some(t));
        }

        if self.function_table.is_overloaded(&base)
            || self.function_table.get_function(&base).is_ok()
        {
            return Ok(Some(self.analyze_static_call(
                &type_name,
                method,
                params,
                ctx.parent_function,
                ctx.symbol_table,
                diagnostics,
            )?));
        }

        Ok(None)
    }
}
