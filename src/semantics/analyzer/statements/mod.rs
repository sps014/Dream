//! Analysis of statements and control flow, split by concern:
//! - [`loops`]: `break`/`continue` placement checks and `while`/`do-while`/`for`/`for-each` loops.
//! - [`case_switch`]: the C-style `switch`/`case` statement (int/string/bool/enum subjects).
//! - [`bindings`]: `let` declarations, simple assignments, and `return`.
//! - [`assignments`]: indexed (`arr[i] = v`) and member (`obj.m = v`) writes and their desugaring.
//! - [`conditionals`]: `if`/`else if`/`else`, including the compile-time `is` fold.
//!
//! The reserved-name check, the bool-condition diagnostic, and the `is`-with-binding flow-typing
//! helpers live here because they are shared across several of those submodules.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::intrinsics;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::{ExpressionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::rc::Rc;

mod assignments;
mod bindings;
mod case_switch;
mod conditionals;
mod loops;

impl<'a> Analyzer<'a> {
    /// Reports a diagnostic when a control-flow condition is not `bool`. Already-`Unknown` conditions
    /// are skipped (their underlying error was reported where the type was produced). `context` names
    /// the construct for the message, e.g. "if" / "while" / "do/while" / "for".
    fn check_bool_condition(
        &self,
        context: &str,
        cond_type: &Type,
        position: Option<TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !cond_type.is_unknown() && !cond_type.is_bool() {
            diagnostics.report_error(
                format!(
                    "{} condition must be bool, got {}",
                    context,
                    cond_type.get_type()
                ),
                position,
            );
        }
    }

    /// Reports a clear diagnostic when a reserved word (a builtin name or primitive type name) is
    /// used where a user-chosen identifier is expected (`role` is e.g. "variable"/"function").
    pub(in crate::semantics::analyzer) fn check_reserved_name(
        &self,
        token: &SyntaxToken,
        role: &str,
        diagnostics: &mut DiagnosticBag,
    ) {
        // bare callable, so it is a legal ordinary identifier.
        const RESERVED_TYPE_AND_LITERAL_NAMES: &[&str] = &[
            "int", "float", "double", "string", "bool", "char", "object", "void", "long", "uint",
            "ulong", "byte",
            // C#/.NET-style aliases for the primitives (see `canonical_type_name`).
            "String", "Int32", "Int64", "UInt32", "UInt64", "Byte", "Single", "Double", "Boolean",
            "Char", "Object", "Void", "true", "false", "null",
        ];
        // The builtin callables are reserved too; sourced from the intrinsic registry so this list
        // never drifts from the set of names the compiler special-cases.
        let is_reserved = RESERVED_TYPE_AND_LITERAL_NAMES.contains(&token.text.as_str())
            || intrinsics::is_object_builtin(&token.text);
        if is_reserved {
            diagnostics.report_error(
                format!(
                    "'{}' is a reserved word and cannot be used as a {} name",
                    token.text, role
                ),
                Some(token.position),
            );
        }
    }

    /// A fresh child symbol scope of `parent`, used for a single `if`/`else if` branch (or a loop
    /// body) so an `is`-with-binding local lives only inside that branch and never leaks to
    /// `else`/the enclosing scope. Outer names stay visible through the parent link.
    fn branch_scope(&self, parent: &Rc<RefCell<SymbolTable>>) -> Rc<RefCell<SymbolTable>> {
        let scope = Rc::new(RefCell::new(SymbolTable::new(Some(Rc::clone(parent)))));
        (*parent).borrow_mut().add_child(scope.clone());
        scope
    }

    /// Collects the `is`-with-binding conditions that are guaranteed true whenever `cond` is true:
    /// a bare `x is T name`, and every such test reachable through a top-level `&&` chain (descending
    /// through parentheses). `||`, negation, and other operators are *not* descended into, because a
    /// binding under them is not guaranteed to hold when the whole condition is true. The collected
    /// bindings are declared into the taken branch (or loop body) by [`Self::declare_is_bindings`].
    fn collect_is_bindings<'e>(
        cond: &'e ExpressionNode<'a>,
        out: &mut Vec<(&'e SyntaxToken, &'e Type, &'e ExpressionNode<'a>)>,
    ) {
        match cond {
            ExpressionNode::IsExpression(left, right_type, Some(name)) => {
                out.push((name, right_type, left));
            }
            ExpressionNode::Parenthesized(inner) => Self::collect_is_bindings(inner, out),
            ExpressionNode::Binary(left, op, right)
                if op.kind == TokenKind::AmpersandAmpersandToken =>
            {
                Self::collect_is_bindings(left, out);
                Self::collect_is_bindings(right, out);
            }
            _ => {}
        }
    }

    /// Declares each collected `is`-binding into `branch_scope` (see [`Self::declare_is_binding`]).
    /// A no-op when there are no bindings. Must be called inside the target block's open HIR block,
    /// before its body, so the narrowed-local declarations lead the block.
    fn declare_is_bindings(
        &mut self,
        bindings: &[(&SyntaxToken, &Type, &ExpressionNode<'a>)],
        branch_scope: &Rc<RefCell<SymbolTable>>,
        ctx: &super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        for &(name, target_ty, operand) in bindings {
            self.declare_is_binding(name, target_ty, operand, branch_scope, ctx, diagnostics)?;
        }
        Ok(())
    }

    /// Introduces an `is`-with-binding local into a branch: it declares `name: T` (added to
    /// `branch_scope` for type-checking) initialized by an implicit `(T)operand` cast. Reusing the
    /// cast path means reference/interface targets alias the same pointer (identity) while value-type
    /// targets (`int`, `bool`, …) unbox the operand — exactly the narrowing `is` implies. Must be
    /// called inside the branch's open HIR block, before its body.
    fn declare_is_binding(
        &mut self,
        name: &SyntaxToken,
        target_ty: &Type,
        operand: &ExpressionNode<'a>,
        branch_scope: &Rc<RefCell<SymbolTable>>,
        ctx: &super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        self.check_reserved_name(name, "variable", diagnostics);
        // Model the narrowed initializer as `(target_ty)operand`, reusing all cast validation +
        // codegen (reference identity vs. primitive unbox).
        let _ = self.analyze_cast(
            target_ty,
            operand,
            ctx.parent_function,
            ctx.symbol_table,
            diagnostics,
        )?;
        let init = self.hir_take();
        self.hir_declare_local(&name.text, target_ty, init);
        if let Err(e) = (*branch_scope)
            .borrow_mut()
            .add_symbol(name.text.clone(), target_ty.clone())
        {
            diagnostics.report_error(e.to_string(), Some(name.position));
        }
        Ok(())
    }
}
