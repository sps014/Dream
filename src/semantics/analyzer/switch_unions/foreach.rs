//! `for (let <element> in <iterable>)` desugaring via the enumerator protocol.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::strip_nullable;
use crate::syntax::nodes::{StatementNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Lowers `for (let <element> in <iterable>)` where `iterable` is a class exposing the
    /// enumerator protocol: an eligible `iterator()` (accessible instance, non-async, 0-arg method
    /// returning an enumerator object) whose `next()` (accessible instance, non-async, 0-arg) yields
    /// `Option<T>`. It desugars to the following, built directly in HIR so that `break`/`continue`
    /// in the user body target this loop (a `switch`/`match` arm would not):
    ///
    /// ```text
    /// let $it = <iterable>.iterator();
    /// while (true) {
    ///     let $opt = $it.next();
    ///     if (discriminant($opt) != Some) { break; }
    ///     <element> = $opt.value;   // the `Some` payload
    ///     <body>
    /// }
    /// ```
    ///
    /// Because `next()` is re-evaluated at the top of every iteration, a `continue` in the body
    /// (which jumps to the loop header) correctly re-advances the iterator. `iter_hir` is the
    /// already-analyzed receiver expression for `iterable`.
    pub(in crate::semantics::analyzer) fn analyze_foreach_iter(
        &mut self,
        element: &SyntaxToken,
        iterable_type: &Type,
        iter_hir: Option<crate::hir::HExpr>,
        body: &[StatementNode<'a>],
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        use crate::hir::{BinOp, HExpr, HExprKind, HStmt};

        // 1. `iterator()`: an eligible 0-arg instance method returning an enumerator object.
        let iterator_info = match self.resolve_hook_or_diagnose(
            iterable_type,
            "iterator",
            0,
            Some(element.position),
            false,
            diagnostics,
            |reason| {
                format!(
                    "type '{}' cannot be iterated: {}",
                    iterable_type.get_type(),
                    reason
                )
            },
            || {
                format!(
                    "for-each can only iterate over arrays or types with an 'iterator()' method, got {}",
                    iterable_type.get_type()
                )
            },
        ) {
            Some(info) => info,
            None => return Ok(()),
        };
        let enumerator_type = match &iterator_info.return_type {
            Some(t) if Self::resolve_struct_parts(t).is_some() => t.clone(),
            _ => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "type '{}' is not iterable: its 'iterator()' must return an enumerator object",
                        iterable_type.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
        };

        // 2. `next()` on the enumerator: an eligible 0-arg instance method returning `Option<T>`.
        let next_info = match self.resolve_hook_or_diagnose(
            &enumerator_type,
            "next",
            0,
            Some(element.position),
            false,
            diagnostics,
            |reason| {
                format!(
                    "enumerator '{}' cannot be iterated: {}",
                    enumerator_type.get_type(),
                    reason
                )
            },
            || {
                format!(
                    "enumerator '{}' must define 'next(): Option<T>' for for-each",
                    enumerator_type.get_type()
                )
            },
        ) {
            Some(info) => info,
            None => return Ok(()),
        };

        let next_ret = next_info.return_type.clone().unwrap_or(Type::Void);
        let opt_args = match Self::resolve_struct_parts(&next_ret) {
            Some((base, args)) if base == "Option" && args.len() == 1 => args,
            _ => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "for-each requires 'next()' to return Option<T>, got {}",
                        next_ret.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
        };

        // Ensure the concrete `Option<T>` layout is registered so its discriminant/field are known.
        self.ensure_union_instantiated("Option", &opt_args, &element.position, diagnostics);
        let opt_key = strip_nullable(&next_ret.get_type()).to_string();
        let some_variant = match self
            .union_table
            .get(&opt_key)
            .and_then(|u| u.variant("Some"))
            .filter(|v| v.fields.len() == 1)
            .cloned()
        {
            Some(v) => v,
            None => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "for-each requires 'next()' to return Option<T>, got {}",
                        next_ret.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
        };
        let element_type = some_variant.fields[0].type_.clone();

        // Claim any label wrapping this loop before the body (which may hold nested loops).
        let label = self.pending_loop_label.take();

        // The user's element binding lives in a dedicated foreach scope.
        let foreach_scope = Rc::new(RefCell::new(SymbolTable::new(Some(
            ctx.symbol_table.clone(),
        ))));
        (*ctx.symbol_table)
            .borrow_mut()
            .add_child(foreach_scope.clone());
        if let Err(e) = foreach_scope
            .borrow_mut()
            .add_symbol(element.text.clone(), element_type.clone())
        {
            diagnostics.report_error(e.to_string(), Some(element.position));
        }

        // HIR locals. `$it`/`$opt` are internal (referenced by id, never by name); the element slot
        // is what the body's identifier references resolve to.
        let it_local = self.hir_alloc_local("$foreach_it", &enumerator_type);
        let opt_local = self.hir_alloc_local("$foreach_opt", &next_ret);
        let elem_slot = self.hir_alloc_local(&element.text, &element_type);

        // `$it = <iterable>.iterator();` (emitted into the enclosing block).
        self.hir_set_method_call(iter_hir, &iterator_info.name, vec![], &enumerator_type);
        let it_call = self.hir_take();
        if let Some(it_l) = it_local {
            self.hir_assign_local_id(it_l, it_call);
        }

        // Loop body.
        self.hir_open_block();
        if let (Some(it_l), Some(opt_l), Some(elem_l)) = (it_local, opt_local, elem_slot) {
            let enum_ty_id = self.type_ctx.lower(&enumerator_type);
            let opt_ty_id = self.type_ctx.lower(&next_ret);
            let union_ty_id = self.type_ctx.interner.strip_nullable(opt_ty_id);
            let field_ty_id = self.type_ctx.lower(&element_type);

            // `$opt = $it.next();`
            let recv = self.hx_local(it_l, enum_ty_id);
            self.hir_set_method_call(Some(recv), &next_info.name, vec![], &next_ret);
            let next_call = self.hir_take();
            self.hir_assign_local_id(opt_l, next_call);

            // `if (discriminant($opt) != Some) { break; }`
            let is_some = self.hx_bin(
                BinOp::Eq,
                self.hx_disc(self.hx_local(opt_l, opt_ty_id)),
                self.hx_int(some_variant.discriminant as i64),
            );
            let break_cond = self.hx_not(is_some);
            self.hir_push_stmt(HStmt::If {
                cond: break_cond,
                then_branch: vec![HStmt::Break(None)],
                else_branch: vec![],
            });

            // `<element> = $opt.value;` (the `Some` payload field).
            let field_expr = HExpr::new(
                field_ty_id,
                HExprKind::UnionField {
                    base: Box::new(self.hx_local(opt_l, opt_ty_id)),
                    union_ty: union_ty_id,
                    variant: some_variant.discriminant as usize,
                    field: 0,
                },
            );
            self.hir_assign_local_id(elem_l, Some(field_expr));
        }

        // The user body is analyzed inside the loop (so `break`/`continue` are valid and target it).
        self.analyze_body(
            body,
            ctx.parent_function,
            Some(&foreach_scope),
            true,
            diagnostics,
        )?;
        let body_hir = self.hir_close_block();

        let true_lit = self.hx_bool(true);
        self.hir_while(Some(true_lit), body_hir, label);
        Ok(())
    }
}
