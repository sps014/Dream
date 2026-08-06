//! `for (let <element> in <iterable>)` desugaring via the enumerator protocol or interface
//! `Collection`/`Iterator` methods.

use super::*;
use dream_diagnostics::DiagnosticBag;
use crate::errors::SemanticError;
use crate::symbol_table::SymbolTable;
use dream_syntax::nodes::{FunctionNode, StatementNode, Type};
use dream_syntax::token::syntax_token::SyntaxToken;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// True when `ty` is an interface-typed `Iterator<…>`, `Collection<…>`, or
    /// `IndexedCollection<…>` (including mangled instances like `Collection_int`).
    pub(in crate::analyzer) fn is_foreach_interface_type(&self, ty: &Type) -> bool {
        let name = ty.get_type();
        if !self.is_interface_name(&name) {
            return false;
        }
        // Prefer demangling the concrete name (`Collection_int` → `Collection`): a mangled
        // `Type::Struct` token may spell the full instance name with no separate generic args.
        let base = self
            .demangle_generic_interface(&name)
            .map(|(b, _)| b)
            .or_else(|| Self::resolve_struct_parts(ty).map(|(b, _)| b))
            .unwrap_or_else(|| name.clone());
        matches!(
            base.as_str(),
            "Iterator" | "Collection" | "IndexedCollection"
        )
    }

    /// Looks up a 0-arg instance method on a concrete interface instance by name.
    fn iface_method_slot(
        &self,
        iface_name: &str,
        method: &str,
    ) -> Option<(usize, &'a FunctionNode<'a>)> {
        let methods = self.interface_methods.get(iface_name)?;
        methods
            .iter()
            .enumerate()
            .find(|(_, m)| m.name.text == method && m.parameters.is_empty())
            .map(|(slot, m)| (slot, *m))
    }

    /// Emits an interface method call with no arguments into `hir.last`.
    fn hir_iface_call0(
        &mut self,
        receiver: Option<dream_hir::HExpr>,
        iface_name: &str,
        method: &FunctionNode<'a>,
        slot: usize,
        ret: &Type,
    ) {
        let iface_id = self.interface_methods.get_index_of(iface_name).unwrap_or(0);
        let sig = self.interface_dispatch_sig(method);
        self.hir_set_interface_call(receiver, iface_id, slot, sig, vec![], ret);
    }

    /// Lowers `for (let <element> in <iterable>)` when `iterable` is typed as `Iterator<T>`,
    /// `Collection<T>`, or `IndexedCollection<T>`. Uses interface dispatch (not `@iterator` /
    /// `@next` protocol hooks):
    ///
    /// ```text
    /// // Collection / IndexedCollection:
    /// let $it = <iterable>.iterator();   // InterfaceCall
    /// // Iterator (receiver is already the enumerator):
    /// let $it = <iterable>;
    /// while (true) {
    ///     let $opt = $it.next();         // InterfaceCall
    ///     if (discriminant($opt) != Some) { break; }
    ///     <element> = $opt.value;
    ///     <body>
    /// }
    /// ```
    pub(in crate::analyzer) fn analyze_foreach_iface(
        &mut self,
        element: &SyntaxToken,
        iterable_type: &Type,
        iter_hir: Option<dream_hir::HExpr>,
        body: &[StatementNode<'a>],
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        use dream_hir::{BinOp, HExpr, HExprKind, HStmt};

        let iface_name = iterable_type.get_type();
        if let Some((base, args)) = Self::resolve_struct_parts(iterable_type) {
            if !args.is_empty() && self.is_generic_interface(&base) {
                self.ensure_interface_instantiated(&base, &args, &element.position, diagnostics);
            }
        } else if let Some((base, arg_str)) = self.demangle_generic_interface(&iface_name) {
            // Parameter types may already be mangled (`Collection_int`); still ensure slots exist.
            let _ = (base, arg_str);
        }

        let base = Self::resolve_struct_parts(iterable_type)
            .map(|(b, _)| b)
            .or_else(|| {
                self.demangle_generic_interface(&iface_name)
                    .map(|(b, _)| b)
            })
            .unwrap_or_else(|| iface_name.clone());

        let (enumerator_type, it_recv_hir) = if base == "Iterator" {
            (iterable_type.clone(), iter_hir)
        } else {
            // Collection / IndexedCollection: `$it = recv.iterator()`.
            let Some((slot, method)) = self.iface_method_slot(&iface_name, "iterator") else {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "interface '{}' has no 0-arg 'iterator' method for for-each",
                        iface_name
                    ),
                    Some(element.position),
                );
                return Ok(());
            };
            let enum_ty = method.return_type.clone().unwrap_or(Type::Unknown);
            if let Some((ebase, eargs)) = Self::resolve_struct_parts(&enum_ty) {
                if !eargs.is_empty() && self.is_generic_interface(&ebase) {
                    self.ensure_interface_instantiated(
                        &ebase,
                        &eargs,
                        &element.position,
                        diagnostics,
                    );
                }
            }
            self.hir_iface_call0(iter_hir, &iface_name, method, slot, &enum_ty);
            let it_call = self.hir_take();
            (enum_ty, it_call)
        };

        let enum_iface = enumerator_type.get_type();
        if let Some((ebase, eargs)) = Self::resolve_struct_parts(&enumerator_type) {
            if !eargs.is_empty() && self.is_generic_interface(&ebase) {
                self.ensure_interface_instantiated(&ebase, &eargs, &element.position, diagnostics);
            }
        }

        let Some((next_slot, next_method)) = self.iface_method_slot(&enum_iface, "next") else {
            self.hir_fail();
            diagnostics.report_error(
                format!(
                    "for-each requires enumerator '{}' to have a 0-arg 'next' method returning Option<T>",
                    enum_iface
                ),
                Some(element.position),
            );
            return Ok(());
        };
        let next_ret = next_method.return_type.clone().unwrap_or(Type::Void);
        let opt_args = match Self::resolve_struct_parts(&next_ret) {
            Some((b, args)) if b == "Option" && args.len() == 1 => args,
            _ => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "for-each requires 'next' to return Option<T>, got {}",
                        next_ret.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
        };

        self.ensure_union_instantiated("Option", &opt_args, &element.position, diagnostics);
        let opt_key = next_ret.get_type();
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
                        "for-each requires 'next' to return Option<T>, got {}",
                        next_ret.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
        };
        let element_type = some_variant.fields[0].type_.clone();

        let label = self.pending_loop_label.take();
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

        let it_local = self.hir_alloc_local("$foreach_it", &enumerator_type);
        let opt_local = self.hir_alloc_local("$foreach_opt", &next_ret);
        let elem_slot = self.hir_alloc_local(&element.text, &element_type);

        if let Some(it_l) = it_local {
            self.hir_assign_local_id(it_l, it_recv_hir);
        }

        self.hir_open_block();
        if let (Some(it_l), Some(opt_l), Some(elem_l)) = (it_local, opt_local, elem_slot) {
            let enum_ty_id = self.type_ctx.lower(&enumerator_type);
            let opt_ty_id = self.type_ctx.lower(&next_ret);
            let field_ty_id = self.type_ctx.lower(&element_type);

            let recv = self.hx_local(it_l, enum_ty_id);
            self.hir_iface_call0(Some(recv), &enum_iface, next_method, next_slot, &next_ret);
            let next_call = self.hir_take();
            self.hir_assign_local_id(opt_l, next_call);

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

            let field_expr = HExpr::new(
                field_ty_id,
                HExprKind::UnionField {
                    base: Box::new(self.hx_local(opt_l, opt_ty_id)),
                    union_ty: opt_ty_id,
                    variant: some_variant.discriminant as usize,
                    field: 0,
                },
            );
            self.hir_assign_local_id(elem_l, Some(field_expr));
        }

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

    /// Lowers `for (let <element> in <iterable>)` where `iterable` is a class exposing the
    /// enumerator protocol: a registered `@iterator` method (0-arg, returning an enumerator
    /// object) whose type has a registered `@next` method yielding `Option<T>`. It desugars to
    /// the following, built directly in HIR so that `break`/`continue` in the user body target
    /// this loop (a `switch`/`match` arm would not):
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
    pub(in crate::analyzer) fn analyze_foreach_iter(
        &mut self,
        element: &SyntaxToken,
        iterable_type: &Type,
        iter_hir: Option<dream_hir::HExpr>,
        body: &[StatementNode<'a>],
        ctx: &super::super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        use dream_hir::{BinOp, HExpr, HExprKind, HStmt};
        use crate::analyzer::declarations::protocol_hooks::ProtocolRole;

        // 1. `@iterator`: an eligible 0-arg instance method returning an enumerator object.
        let (_iterator_hook, iterator_info) = match self.resolve_hook_or_diagnose(
            iterable_type,
            ProtocolRole::Iterator,
            Some(element.position),
            false,
            diagnostics,
            || {
                format!(
                    "for-each can only iterate over arrays or types with an '@iterator' method, got {}",
                    iterable_type.get_type()
                )
            },
        ) {
            Some(resolved) => resolved,
            None => return Ok(()),
        };
        let enumerator_type = match &iterator_info.return_type {
            Some(t) if Self::resolve_struct_parts(t).is_some() => t.clone(),
            _ => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "type '{}' is not iterable: its '@iterator' method must return an enumerator object",
                        iterable_type.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
        };

        // 2. `@next` on the enumerator: an eligible 0-arg instance method returning `Option<T>`.
        let (_next_hook, next_info) = match self.resolve_hook_or_diagnose(
            &enumerator_type,
            ProtocolRole::Next,
            Some(element.position),
            false,
            diagnostics,
            || {
                format!(
                    "enumerator '{}' must define '@next fun ...(): Option<T>' for for-each",
                    enumerator_type.get_type()
                )
            },
        ) {
            Some(resolved) => resolved,
            None => return Ok(()),
        };

        let next_ret = next_info.return_type.clone().unwrap_or(Type::Void);
        let opt_args = match Self::resolve_struct_parts(&next_ret) {
            Some((base, args)) if base == "Option" && args.len() == 1 => args,
            _ => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "for-each requires '@next' to return Option<T>, got {}",
                        next_ret.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
        };

        // Ensure the concrete `Option<T>` layout is registered so its discriminant/field are known.
        self.ensure_union_instantiated("Option", &opt_args, &element.position, diagnostics);
        let opt_key = next_ret.get_type();
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
                        "for-each requires '@next' to return Option<T>, got {}",
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
            let union_ty_id = opt_ty_id;
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
