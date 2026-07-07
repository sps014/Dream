//! `expr as T` cast validation and the `compare_data_type` assignability check that backs
//! assignments, argument passing, and comparisons.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::{is_numeric_primitive, strip_nullable};
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::text::text_span::TextSpan;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Types a cast `expr as T`: instantiates a generic target struct if needed, then validates the
    /// conversion (identity, numeric<->numeric, `char`<->`int`/`byte`, boxing/unboxing via `object`,
    /// and `int`->pointer for null literals). Always yields the target type, reporting an error for
    /// disallowed conversions so analysis can continue.
    pub(in crate::semantics::analyzer) fn analyze_cast(
        &mut self,
        target_type: &Type,
        expr: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let expr_type =
            self.analyze_expression(expr, parent_function, symbol_table, diagnostics)?;
        let inner_hir = self.hir_take();

        let target_type_str = target_type.get_type();
        let expr_type_str = expr_type.get_type();

        // If the target (after peeling array wrappers) is a generic struct, instantiate it.
        let mut core_target = target_type;
        while let Type::Array(inner) = core_target {
            core_target = inner;
        }
        if let Some((base_name, generic_args)) = Self::resolve_struct_parts(core_target) {
            self.ensure_struct_instantiated(&base_name, &generic_args, &empty_span(), diagnostics);
        }

        // The cast yields `target_type` regardless of whether the conversion is allowed (a
        // disallowed one is reported below); record its HIR before the validation branches.
        self.hir_set_cast(inner_hir, target_type);

        if target_type_str == expr_type_str ||
           (is_numeric_primitive(&target_type_str) && is_numeric_primitive(&expr_type_str)) ||
           // `char` is a code point: allow lossless conversion to/from `int`/`byte`.
           (target_type_str == "char" && (expr_type_str == "int" || expr_type_str == "byte")) ||
           ((target_type_str == "int" || target_type_str == "byte") && expr_type_str == "char")
        {
            Ok(target_type.clone())
        } else if target_type_str == "object" || expr_type_str == "object" {
            // Boxing (`T as object`) and unboxing (`object as T`) are always permitted;
            // an unbox to the wrong primitive traps at runtime.
            Ok(target_type.clone())
        } else if expr_type_str == "int"
            && (self.struct_table.get_struct(&target_type_str).is_some()
                || target_type_str.ends_with("[]")
                || target_type_str.ends_with("?"))
        {
            // Allow casting int to pointer types (for null pointers)
            Ok(target_type.clone())
        } else if self.is_interface_name(strip_nullable(&target_type_str)) {
            // Cast to an interface (`(Animal)cat`). Allowed from another interface, or a class that
            // implements the interface (an upcast). Both are identity at runtime (same tagged
            // pointer); only the static type changes.
            let src = strip_nullable(&expr_type_str);
            if self.is_interface_name(src)
                || self.implements_as_interface_ref(src, strip_nullable(&target_type_str))
            {
                Ok(target_type.clone())
            } else {
                diagnostics.report_error(
                    format!(
                        "Cannot cast from {} to interface {} ({} does not implement it)",
                        expr_type_str, target_type_str, expr_type_str
                    ),
                    target_type.get_span().or_else(|| expr.position()),
                );
                Ok(target_type.clone())
            }
        } else if self.is_interface_name(strip_nullable(&expr_type_str)) {
            // Downcast from an interface to a concrete class or another interface: permitted
            // (identity at runtime; like unboxing `object`, a wrong downcast is the caller's risk).
            Ok(target_type.clone())
        } else {
            diagnostics.report_error(
                format!("Cannot cast from {} to {}", expr_type_str, target_type_str),
                target_type.get_span().or_else(|| expr.position()),
            );
            Ok(target_type.clone())
        }
    }

    pub(in crate::semantics::analyzer) fn compare_data_type(
        &mut self,
        left: &Type,
        right: &Type,
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        // A poison operand (from an earlier reported error) is compatible with anything, so we
        // never emit a follow-on mismatch for it.
        if left.is_unknown() || right.is_unknown() {
            return Ok(());
        }

        // Directional assignability over interned types: `right` (value) must be assignable to
        // `left` (target). Covers identity, `object` widening, enum/int, numeric widening, and
        // nullable/`null` handling via the structured rules.
        let l = self.type_ctx.lower(left);
        let r = self.type_ctx.lower(right);
        if crate::types::assignable(&self.type_ctx.interner, l, r) {
            return Ok(());
        }
        // `compare_data_type` also backs equality comparisons (`ref == null`, `null == ref`),
        // where `null` may appear on either side. Accept the reverse direction, but only for the
        // `null`-literal case so a narrowing assignment is still rejected.
        if (left.get_type() == "void?" || right.get_type() == "void?")
            && crate::types::assignable(&self.type_ctx.interner, r, l)
        {
            return Ok(());
        }

        // Implicit upcast to an interface: a value whose concrete class implements the interface
        // `left` is assignable to it (`let a: Animal = cat;`).
        if self.value_assignable_to_interface(left, right) {
            return Ok(());
        }

        diagnostics.report_error(
            format!(
                "cannot convert from {} to {} at {}",
                left.display_name(),
                right.display_name(),
                position.get_point_str()
            ),
            Some(*position),
        );
        Ok(())
    }
}
