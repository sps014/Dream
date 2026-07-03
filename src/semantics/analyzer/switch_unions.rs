//! Analysis of discriminated-union construction (`Enum.Variant(args)` / unit `Enum.Variant`) and
//! of pattern-matching `switch` expressions/statements: pattern typing, binding scopes, guards,
//! arm-type unification, exhaustiveness, and unreachable-arm detection.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::semantics::union_table::UnionInfo;
use crate::syntax::nodes::types::strip_nullable;
use crate::syntax::nodes::{
    ExpressionNode, FunctionNode, PatternNode, StatementNode, SwitchArm, SwitchArmBody, Type,
};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The HIR shape a switch pattern lowers to (for statement-position `switch` → [`HStmt::Switch`]).
enum HirArmShape {
    /// A `Const` arm (literal pattern).
    Const(crate::hir::HExpr),
    /// A `Variant` arm; `bindings` are the payload local slots in field order.
    Variant {
        def: crate::types::DefId,
        variant: usize,
        bindings: Vec<crate::hir::LocalId>,
    },
    /// A catch-all `_` → the switch `default` block.
    Default,
    /// A catch-all that binds the whole subject to `local` (a bare identifier naming no variant) →
    /// the `default` block, prefixed with `let <name> = <subject>;`.
    DefaultBind {
        local: crate::hir::LocalId,
        ty: crate::types::TypeId,
    },
    /// Not representable in HIR's `Switch` (nested sub-pattern, bad literal).
    Unsupported,
}

/// What checking a single pattern told us, used to drive exhaustiveness and unreachable-arm
/// analysis.
struct PatternInfo {
    /// True when the pattern matches every value of its type (a bare binding or `_`). Drives
    /// unreachable-arm detection; full (possibly nested) coverage is computed separately in
    /// [`Analyzer::check_exhaustiveness`] from the arm patterns.
    irrefutable: bool,
}

impl<'a> Analyzer<'a> {
    /// If `enum_name` denotes a discriminated union (concrete or generic) and `variant` names one
    /// of its variants, type-checks the construction `Enum.Variant(args)` and returns its type.
    /// Returns `Ok(None)` when `enum_name` is not a union (so the caller can fall through to its
    /// normal handling, e.g. C-style enum member access or a static method call).
    pub(super) fn analyze_variant_construction(
        &mut self,
        enum_name: &str,
        variant: &SyntaxToken,
        args: &[ExpressionNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Option<Type>, SemanticError> {
        let is_generic = self.generic_unions.contains_key(enum_name);
        let is_concrete = self.union_table.contains_key(enum_name);
        if !is_generic && !is_concrete {
            return Ok(None);
        }

        // The declared payload field types of the named variant (templated for generic unions).
        let field_types: Vec<Type> = if is_generic {
            let template = *self.generic_unions.get(enum_name).unwrap();
            match template
                .variants
                .iter()
                .find(|v| v.name.text == variant.text)
            {
                Some(v) => v.fields.iter().map(|f| f.field_type.clone()).collect(),
                None => {
                    return Err(report(
                        diagnostics,
                        format!("Enum '{}' has no variant '{}'", enum_name, variant.text),
                        Some(variant.position),
                    ));
                }
            }
        } else {
            let info = match self.union_table.get(enum_name) {
                Some(info) => info,
                None => {
                    return Err(report(
                        diagnostics,
                        format!("Enum '{}' could not be resolved", enum_name),
                        Some(variant.position.clone()),
                    ));
                }
            };
            match info.variant(&variant.text) {
                Some(v) => v.fields.iter().map(|f| f.type_.clone()).collect(),
                None => {
                    return Err(report(
                        diagnostics,
                        format!("Enum '{}' has no variant '{}'", enum_name, variant.text),
                        Some(variant.position),
                    ));
                }
            }
        };

        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        for arg in args {
            let t = self.analyze_expression(arg, parent_function, symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
            arg_types.push(t);
        }

        if !is_generic {
            self.validate_variant_payload(enum_name, &variant.text, &field_types, &arg_types, variant.position.clone(), diagnostics);
            let result_ty =
                Type::Struct(synthetic_token(TokenKind::IdentifierToken, enum_name), None);
            // Construct the union value: resolve its `DefId` and the variant's discriminant.
            let def = self.type_ctx.defs.lookup(crate::types::DefKind::Union, enum_name);
            let disc = self
                .union_table
                .get(enum_name)
                .and_then(|i| i.variant(&variant.text))
                .map(|v| v.discriminant as usize);
            match (def, disc) {
                (Some(def), Some(disc)) => {
                    self.hir_set_union_new(def, disc, arg_hirs, &result_ty)
                }
                _ => self.hir_none(),
            }
            return Ok(Some(result_ty));
        }

        // Generic union: resolve the concrete type arguments, preferring an explicit expected type
        // (e.g. a `let`/`return` annotation) and otherwise inferring from the arguments.
        let template = *self.generic_unions.get(enum_name).unwrap();
        let params: Vec<String> = template
            .generic_parameters
            .as_ref()
            .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
            .unwrap_or_default();

        let mut concrete_args: Option<Vec<Type>> = None;
        if let Some(Type::Struct(b, Some(eargs))) = &self.current_expected_type {
            if b.text == enum_name && eargs.len() == params.len() {
                concrete_args = Some(eargs.clone());
            }
        }
        if concrete_args.is_none() {
            let mut binding: HashMap<String, Type> = HashMap::new();
            for (ft, at) in field_types.iter().zip(arg_types.iter()) {
                let name = ft.get_type();
                if params.contains(&name) {
                    binding.entry(name).or_insert_with(|| at.clone());
                }
            }
            let resolved: Vec<Type> = params
                .iter()
                .filter_map(|p| binding.get(p).cloned())
                .collect();
            if resolved.len() == params.len() {
                concrete_args = Some(resolved);
            }
        }

        let concrete_args = match concrete_args {
            Some(a) => a,
            None => {
                return Err(report(
                    diagnostics,
                    format!(
                        "Cannot infer type arguments for '{}.{}'; add a type annotation (e.g. `let x: {}<...> = ...`)",
                        enum_name, variant.text, enum_name
                    ),
                    Some(variant.position),
                ));
            }
        };

        let bindings = generic_bindings(
            template.generic_parameters.as_deref().unwrap_or(&[]),
            &concrete_args,
        );
        let expected_fields: Vec<Type> = field_types.iter().map(|ft| substitute_generic_type(ft, &bindings)).collect();
        self.validate_variant_payload(enum_name, &variant.text, &expected_fields, &arg_types, variant.position.clone(), diagnostics);

        self.ensure_union_instantiated(enum_name, &concrete_args, &variant.position, diagnostics);
        // Construct the monomorphized union value. Its interned type (`union_ty(def, args)`) matches
        // the layout keyed by the mangled instance, so the backend resolves variant offsets. The
        // shared template `DefId` + the discriminant from the concrete instance name select the arm.
        let result_ty = Type::Struct(
            synthetic_token(TokenKind::IdentifierToken, enum_name),
            Some(concrete_args),
        );
        let mangled = crate::syntax::nodes::types::mangle_generic(
            enum_name,
            match &result_ty {
                Type::Struct(_, Some(a)) => a,
                _ => unreachable!(),
            },
        );
        let def = self.type_ctx.defs.lookup(crate::types::DefKind::Union, enum_name);
        let disc = self
            .union_table
            .get(&mangled)
            .and_then(|i| i.variant(&variant.text))
            .map(|v| v.discriminant as usize);
        match (def, disc) {
            (Some(def), Some(disc)) => self.hir_set_union_new(def, disc, arg_hirs, &result_ty),
            _ => self.hir_none(),
        }
        Ok(Some(result_ty))
    }

    /// Classifies a switch pattern for HIR statement-`switch` lowering, allocating HIR locals for any
    /// variant-payload bindings *before* the arm body is lowered so the body can resolve them.
    fn hir_switch_pattern(
        &mut self,
        pattern: &PatternNode,
        union_info: &Option<UnionInfo>,
        union_def: Option<crate::types::DefId>,
        subject_type: &Type,
    ) -> HirArmShape {
        match pattern {
            PatternNode::Wildcard(_) => HirArmShape::Default,
            PatternNode::Binding(name) => {
                // A bare identifier naming a unit variant is a unit-variant pattern; otherwise it
                // binds the whole subject and acts as a catch-all `default` arm (the subject value
                // is copied into the named local, injected by the caller).
                if let (Some(info), Some(def)) = (union_info, union_def) {
                    if let Some(v) = info.variant(&name.text) {
                        if v.fields.is_empty() {
                            return HirArmShape::Variant {
                                def,
                                variant: v.discriminant as usize,
                                bindings: vec![],
                            };
                        }
                    }
                }
                let ty = self.type_ctx.lower(subject_type);
                match self.hir_alloc_local(&name.text, subject_type) {
                    Some(local) => HirArmShape::DefaultBind { local, ty },
                    None => HirArmShape::Unsupported,
                }
            }
            PatternNode::Literal(lit) => {
                self.hir_set_literal(lit);
                match self.hir_take() {
                    Some(e) => HirArmShape::Const(e),
                    None => HirArmShape::Unsupported,
                }
            }
            PatternNode::Variant(_, name, subs) => {
                let (Some(info), Some(def)) = (union_info, union_def) else {
                    return HirArmShape::Unsupported;
                };
                let Some(v) = info.variant(&name.text) else {
                    return HirArmShape::Unsupported;
                };
                if subs.len() != v.fields.len() {
                    return HirArmShape::Unsupported;
                }
                let fields: Vec<(String, Type)> = v
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_.clone()))
                    .collect();
                let variant = v.discriminant as usize;
                let mut bindings = Vec::with_capacity(subs.len());
                for (i, sub) in subs.iter().enumerate() {
                    // Only flat `Binding`/`_` sub-patterns are representable; each field gets a slot.
                    let (slot_name, fty) = match sub {
                        PatternNode::Binding(bn) => (bn.text.clone(), fields[i].1.clone()),
                        PatternNode::Wildcard(_) => {
                            (format!("__switch_{}_{}", variant, i), fields[i].1.clone())
                        }
                        _ => return HirArmShape::Unsupported,
                    };
                    match self.hir_alloc_local(&slot_name, &fty) {
                        Some(id) => bindings.push(id),
                        None => return HirArmShape::Unsupported,
                    }
                }
                HirArmShape::Variant {
                    def,
                    variant,
                    bindings,
                }
            }
        }
    }

    /// True when `arms` need the general if-chain lowering rather than a `Switch`: any arm has a
    /// guard, or a variant pattern has a non-flat sub-pattern (a literal or a nested variant).
    fn pattern_switch_needs_chain(arms: &[SwitchArm]) -> bool {
        arms.iter().any(|a| a.guard.is_some() || Self::pattern_is_nested(&a.pattern))
    }

    /// True for a variant pattern with at least one sub-pattern that isn't a flat binding/wildcard.
    fn pattern_is_nested(p: &PatternNode) -> bool {
        matches!(p, PatternNode::Variant(_, _, subs)
            if subs.iter().any(|s| !matches!(s, PatternNode::Binding(_) | PatternNode::Wildcard(_))))
    }

    // -- small typed-HExpr builders used by the if-chain switch lowering --
    fn hx_bool(&self, v: bool) -> crate::hir::HExpr {
        crate::hir::HExpr::new(self.type_ctx.interner.bool(), crate::hir::HExprKind::BoolLit(v))
    }
    fn hx_int(&self, v: i64) -> crate::hir::HExpr {
        crate::hir::HExpr::new(self.type_ctx.interner.int(), crate::hir::HExprKind::IntLit(v))
    }
    fn hx_local(&self, local: crate::hir::LocalId, ty: crate::types::TypeId) -> crate::hir::HExpr {
        crate::hir::HExpr::new(ty, crate::hir::HExprKind::Var(crate::hir::Binding::Local(local)))
    }
    fn hx_disc(&self, v: crate::hir::HExpr) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.int(),
            crate::hir::HExprKind::Discriminant(Box::new(v)),
        )
    }
    fn hx_bin(&self, op: crate::hir::BinOp, a: crate::hir::HExpr, b: crate::hir::HExpr) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.bool(),
            crate::hir::HExprKind::Binary { op, lhs: Box::new(a), rhs: Box::new(b) },
        )
    }
    fn hx_not(&self, a: crate::hir::HExpr) -> crate::hir::HExpr {
        crate::hir::HExpr::new(
            self.type_ctx.interner.bool(),
            crate::hir::HExprKind::Unary { op: crate::hir::UnOp::Not, operand: Box::new(a) },
        )
    }

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
    pub(super) fn analyze_foreach_iter(
        &mut self,
        element: &SyntaxToken,
        iterable_type: &Type,
        iter_hir: Option<crate::hir::HExpr>,
        body: &[StatementNode<'a>],
        ctx: &super::AnalyzerContext<'a, '_>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        use super::calls::HookResolution;
        use crate::hir::{BinOp, HExpr, HExprKind, HStmt};

        // 1. `iterator()`: an eligible 0-arg instance method returning an enumerator object.
        let iterator_info = match self.resolve_hook_method(iterable_type, "iterator", 0, diagnostics)
        {
            HookResolution::Eligible(info) => info,
            HookResolution::Ineligible(reason) => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "type '{}' cannot be iterated: {}",
                        iterable_type.get_type(),
                        reason
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
            HookResolution::Absent => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "for-each can only iterate over arrays or types with an 'iterator()' method, got {}",
                        iterable_type.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
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
        let next_info = match self.resolve_hook_method(&enumerator_type, "next", 0, diagnostics) {
            HookResolution::Eligible(info) => info,
            HookResolution::Ineligible(reason) => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "enumerator '{}' cannot be iterated: {}",
                        enumerator_type.get_type(),
                        reason
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
            HookResolution::Absent => {
                self.hir_fail();
                diagnostics.report_error(
                    format!(
                        "enumerator '{}' must define 'next(): Option<T>' for for-each",
                        enumerator_type.get_type()
                    ),
                    Some(element.position),
                );
                return Ok(());
            }
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
        let foreach_scope = Rc::new(RefCell::new(SymbolTable::new(Some(ctx.symbol_table.clone()))));
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
        self.analyze_body(body, ctx.parent_function, Some(&foreach_scope), true, diagnostics)?;
        let body_hir = self.hir_close_block();

        let true_lit = self.hx_bool(true);
        self.hir_while(Some(true_lit), body_hir, label);
        Ok(())
    }

    /// Recursively compiles `pattern` (matched against value `value` of type `value_type`) into a set
    /// of boolean test conditions plus named payload bindings. Returns `None` if the pattern isn't
    /// representable (so the caller drops the function). All field reads are inlined into the returned
    /// expressions, so the conditions/bindings are self-contained (no reliance on prior bindings).
    #[allow(clippy::type_complexity)]
    fn compile_pattern(
        &mut self,
        value: &crate::hir::HExpr,
        value_type: &Type,
        pattern: &PatternNode,
    ) -> Option<(Vec<crate::hir::HExpr>, Vec<(String, Type, crate::hir::HExpr)>)> {
        use crate::hir::{BinOp, HExpr, HExprKind};
        let base = strip_nullable(&value_type.get_type()).to_string();
        match pattern {
            PatternNode::Wildcard(_) => Some((vec![], vec![])),
            PatternNode::Binding(name) => {
                // A bare identifier naming a unit variant of the value's union is a variant test;
                // otherwise it binds the whole value.
                if let Some(info) = self.union_table.get(&base).cloned() {
                    if let Some(v) = info.variant(&name.text) {
                        if v.fields.is_empty() {
                            let cond = self.hx_bin(
                                BinOp::Eq,
                                self.hx_disc(value.clone()),
                                self.hx_int(v.discriminant as i64),
                            );
                            return Some((vec![cond], vec![]));
                        }
                    }
                }
                Some((vec![], vec![(name.text.clone(), value_type.clone(), value.clone())]))
            }
            PatternNode::Literal(lit) => {
                self.hir_set_literal(lit);
                let le = self.hir_take()?;
                Some((vec![self.hx_bin(BinOp::Eq, value.clone(), le)], vec![]))
            }
            PatternNode::Variant(_qual, name, subs) => {
                let info = self.union_table.get(&base).cloned()?;
                let v = info.variant(&name.text)?.clone();
                if subs.len() != v.fields.len() {
                    return None;
                }
                let lowered = self.type_ctx.lower(value_type);
                let union_ty_id = self.type_ctx.interner.strip_nullable(lowered);
                let mut conds = vec![self.hx_bin(
                    BinOp::Eq,
                    self.hx_disc(value.clone()),
                    self.hx_int(v.discriminant as i64),
                )];
                let mut binds = Vec::new();
                for (i, sub) in subs.iter().enumerate() {
                    let fty = v.fields[i].type_.clone();
                    let fty_id = self.type_ctx.lower(&fty);
                    let field_expr = HExpr::new(
                        fty_id,
                        HExprKind::UnionField {
                            base: Box::new(value.clone()),
                            union_ty: union_ty_id,
                            variant: v.discriminant as usize,
                            field: i,
                        },
                    );
                    let (mut c, mut b) = self.compile_pattern(&field_expr, &fty, sub)?;
                    conds.append(&mut c);
                    binds.append(&mut b);
                }
                Some((conds, binds))
            }
        }
    }

    /// General pattern-`switch` lowering (guards + nested/literal sub-patterns) as a flag-gated
    /// if-chain: evaluates the subject once, then for each arm emits `if (!done && <tests>) {
    /// <binds>; [if (<guard>)] { done = true; <body> } }`. A failed guard leaves `done` false so the
    /// next arm is tried. Type-checking (pattern checks, guard/body analysis, exhaustiveness) mirrors
    /// the `Switch` path.
    fn analyze_pattern_switch_chain(
        &mut self,
        subject: &ExpressionNode<'a>,
        arms: &[SwitchArm<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        use crate::hir::{BinOp, HStmt};

        let subject_type =
            self.analyze_expression(subject, parent_function, symbol_table, diagnostics)?;
        let subject_hir = self.hir_take();
        if let Type::Struct(base, Some(args)) = &subject_type {
            if self.generic_unions.contains_key(&base.text) {
                self.ensure_union_instantiated(&base.text, args, &base.position, diagnostics);
            }
        }
        let subject_base = strip_nullable(&subject_type.get_type()).to_string();
        let union_info: Option<UnionInfo> = self.union_table.get(&subject_base).cloned();

        let bool_type = Type::Boolean(synthetic_token(TokenKind::BooleanToken, "bool"));
        let bool_ty = self.type_ctx.interner.bool();
        let subj_ty_id = self.type_ctx.lower(&subject_type);

        let mut emit_ok = subject_hir.is_some();

        // Bind the subject once and initialize the `done` flag.
        let subj_local = self.hir_alloc_local("__switch_subj", &subject_type);
        match (subj_local, subject_hir) {
            (Some(local), Some(sh)) => {
                self.hir_push_stmt(HStmt::Let { local, ty: subj_ty_id, value: sh })
            }
            _ => emit_ok = false,
        }
        let done_local = self.hir_alloc_local("__switch_done", &bool_type);
        if let Some(done) = done_local {
            let init = self.hx_bool(false);
            self.hir_push_stmt(HStmt::Let { local: done, ty: bool_ty, value: init });
        }

        let subj_read = |s: &Self| s.hx_local(subj_local.unwrap_or(crate::hir::LocalId(0)), subj_ty_id);

        let mut arm_value_type: Option<Type> = None;
        let mut catch_all_index: Option<usize> = None;
        let mut result_temp: Option<crate::hir::LocalId> = None;
        let mut result_ty_id: Option<crate::types::TypeId> = None;

        for (i, arm) in arms.iter().enumerate() {
            if catch_all_index.is_some() {
                diagnostics.report_error(
                    "Unreachable switch arm: a previous arm already matches everything".to_string(),
                    arm.pattern.position(),
                );
            }

            let arm_scope = Rc::new(RefCell::new(SymbolTable::new(Some(symbol_table.clone()))));
            (*symbol_table).borrow_mut().add_child(arm_scope.clone());

            let info = self.check_pattern(&arm.pattern, &subject_type, &arm_scope, diagnostics)?;

            // Build the arm's tests + payload bindings from the resolved subject value.
            let sr = subj_read(self);
            let (conds, binds) = match self.compile_pattern(&sr, &subject_type, &arm.pattern) {
                Some(cb) => cb,
                None => {
                    emit_ok = false;
                    (vec![], vec![])
                }
            };

            // then-branch: declare bindings, then (optionally guard and) run the body.
            self.hir_open_block();
            for (name, ty, expr) in binds {
                self.hir_declare_local(&name, &ty, Some(expr));
            }

            let mut run_body = |s: &mut Self, diags: &mut DiagnosticBag| -> Result<(), SemanticError> {
                if let Some(done) = done_local {
                    let t = s.hx_bool(true);
                    s.hir_assign_local_id(done, Some(t));
                }
                match &arm.body {
                    SwitchArmBody::Expr(expr) => {
                        let t = s.analyze_expression(expr, parent_function, &arm_scope, diags)?;
                        let arm_hir = s.hir_take();
                        if is_expression {
                            match &arm_value_type {
                                None => arm_value_type = Some(t.clone()),
                                Some(prev) => s.compare_data_type(prev, &t, &empty_span(), diags)?,
                            }
                            if result_temp.is_none() {
                                result_temp = s.hir_alloc_local("__switch_result", &t);
                                result_ty_id = Some(s.type_ctx.lower(&t));
                            }
                            match result_temp {
                                Some(tmp) => s.hir_assign_local_id(tmp, arm_hir),
                                None => emit_ok = false,
                            }
                        } else {
                            s.hir_expr_stmt(arm_hir);
                        }
                    }
                    SwitchArmBody::Block(stmts) => {
                        if is_expression {
                            diags.report_error(
                                "A block arm (`=> { ... }`) is only allowed when `switch` is used as a statement; use `=> expr` in expression position".to_string(),
                                arm.pattern.position(),
                            );
                        }
                        s.analyze_body(stmts, parent_function, Some(&arm_scope), false, diags)?;
                    }
                }
                Ok(())
            };

            if let Some(guard) = &arm.guard {
                let gt = self.analyze_expression(guard, parent_function, &arm_scope, diagnostics)?;
                let guard_hir = self.hir_take();
                if !gt.is_unknown() && !gt.is_bool() {
                    diagnostics.report_error(
                        format!("switch guard must be a bool, got {}", gt.get_type()),
                        guard.position(),
                    );
                }
                self.hir_open_block();
                run_body(self, diagnostics)?;
                let guard_then = self.hir_close_block();
                match guard_hir {
                    Some(g) => self.hir_push_stmt(HStmt::If {
                        cond: g,
                        then_branch: guard_then,
                        else_branch: vec![],
                    }),
                    None => emit_ok = false,
                }
            } else {
                run_body(self, diagnostics)?;
            }
            let then_branch = self.hir_close_block();

            // cond = !done && conds[0] && conds[1] ...
            let mut cond = self.hx_not(self.hx_local(done_local.unwrap_or(crate::hir::LocalId(0)), bool_ty));
            for c in conds {
                cond = self.hx_bin(BinOp::And, cond, c);
            }
            self.hir_push_stmt(HStmt::If { cond, then_branch, else_branch: vec![] });

            // Track the first irrefutable (catch-all) arm so later arms can be flagged unreachable.
            // (Exhaustiveness itself is decided from the arm patterns in `check_exhaustiveness`.)
            if arm.guard.is_none() && info.irrefutable {
                catch_all_index = Some(i);
            }
        }

        if is_expression {
            match (result_temp, result_ty_id) {
                (Some(tmp), Some(ty)) if emit_ok => self.hir_set_local_read(tmp, ty),
                _ => {
                    self.hir_fail();
                    self.hir_none();
                }
            }
        } else if !emit_ok {
            self.hir_fail();
        }

        // Exhaustiveness: a guarded catch-all doesn't count, so require full variant coverage or `_`
        // (recursively, so nested patterns like `Wrap(A(n))` + `Wrap(B)` count as covering `Wrap`).
        self.check_exhaustiveness(
            &subject_base,
            &subject_type,
            &union_info,
            arms,
            subject.position(),
            diagnostics,
        );

        if is_expression {
            Ok(arm_value_type.unwrap_or(Type::Void))
        } else {
            Ok(Type::Void)
        }
    }

    /// Analyzes a pattern-matching `switch`. `is_expression` is true when the switch is used in
    /// value position (all arms must be `=> expr` and share one type); false in statement position
    /// (block arms are allowed and the result is `void`). Returns the unified arm type (or `void`).
    pub(super) fn analyze_pattern_switch(
        &mut self,
        subject: &ExpressionNode<'a>,
        arms: &[SwitchArm<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        is_expression: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        // A `Switch` (br_table) can only express flat, unguarded variant/const arms. Switches with a
        // guard or a nested/literal sub-pattern lower through the general if-chain path instead.
        if Self::pattern_switch_needs_chain(arms) {
            return self.analyze_pattern_switch_chain(
                subject,
                arms,
                parent_function,
                symbol_table,
                is_expression,
                diagnostics,
            );
        }
        let subject_type =
            self.analyze_expression(subject, parent_function, symbol_table, diagnostics)?;
        let subject_hir = self.hir_take();
        // The subject's union may be a generic instantiation that has not been constructed yet
        // (e.g. matching on a `param: Option<int>`); ensure its layout is registered first.
        if let Type::Struct(base, Some(args)) = &subject_type {
            if self.generic_unions.contains_key(&base.text) {
                self.ensure_union_instantiated(&base.text, args, &base.position, diagnostics);
            }
        }
        let subject_base = strip_nullable(&subject_type.get_type()).to_string();
        let union_info: Option<UnionInfo> = self.union_table.get(&subject_base).cloned();
        let union_def = self
            .type_ctx
            .defs
            .lookup(crate::types::DefKind::Union, &subject_base);

        // A whole-subject binding arm (`other => ...`, where `other` names no unit variant) needs the
        // subject value available in the `default` block. Bind it to a temp once and dispatch the
        // `Switch` on a read of that temp, so the binding arm can copy it into its named local.
        let subj_ty_id = self.type_ctx.lower(&subject_type);
        let has_whole_bind = arms.iter().any(|a| {
            a.guard.is_none()
                && matches!(&a.pattern, PatternNode::Binding(n)
                    if !matches!(&union_info, Some(info) if info.variant(&n.text).is_some_and(|v| v.fields.is_empty())))
        });
        let switch_scrutinee = if has_whole_bind {
            match (self.hir_alloc_local("__switch_subj", &subject_type), subject_hir) {
                (Some(subj_local), Some(sh)) => {
                    self.hir_push_stmt(crate::hir::HStmt::Let {
                        local: subj_local,
                        ty: subj_ty_id,
                        value: sh,
                    });
                    Some(self.hx_local(subj_local, subj_ty_id))
                }
                _ => None,
            }
        } else {
            subject_hir
        };

        let mut arm_value_type: Option<Type> = None;
        let mut catch_all_index: Option<usize> = None;

        // HIR: build `Switch` arms + a default block. A statement-position switch lowers directly; a
        // value-position switch desugars to `<result temp> = arm; … ; <result temp read>`, with each
        // arm body assigning the shared result temporary.
        let mut hir_arms: Vec<crate::hir::HArm> = Vec::new();
        let mut hir_default: Vec<crate::hir::HStmt> = Vec::new();
        let mut hir_ok = switch_scrutinee.is_some();
        let mut result_temp: Option<crate::hir::LocalId> = None;
        let mut result_ty_id: Option<crate::types::TypeId> = None;

        for (i, arm) in arms.iter().enumerate() {
            if catch_all_index.is_some() {
                diagnostics.report_error(
                    "Unreachable switch arm: a previous arm already matches everything".to_string(),
                    arm.pattern.position(),
                );
            }

            // Each arm introduces its pattern bindings into a fresh child scope.
            let arm_scope = Rc::new(RefCell::new(SymbolTable::new(Some(symbol_table.clone()))));
            (*symbol_table).borrow_mut().add_child(arm_scope.clone());

            let info = self.check_pattern(&arm.pattern, &subject_type, &arm_scope, diagnostics)?;

            if let Some(guard) = &arm.guard {
                // HIR `Switch` arms have no guard, so a guarded arm drops the function out of coverage.
                hir_ok = false;
                let gt =
                    self.analyze_expression(guard, parent_function, &arm_scope, diagnostics)?;
                if !gt.is_unknown() && !gt.is_bool() {
                    diagnostics.report_error(
                        format!("switch guard must be a bool, got {}", gt.get_type()),
                        guard.position(),
                    );
                }
            }

            // Classify the pattern (allocating payload binding slots) before the body is lowered.
            let shape = self.hir_switch_pattern(&arm.pattern, &union_info, union_def, &subject_type);

            self.hir_open_block();
            // A whole-subject binding copies the subject into its named local as the first statement
            // of the (catch-all) body, mirroring the `Switch` scrutinee read.
            if let HirArmShape::DefaultBind { local, ty } = &shape {
                let read = self.hx_local(
                    match &switch_scrutinee {
                        Some(crate::hir::HExpr {
                            kind: crate::hir::HExprKind::Var(crate::hir::Binding::Local(l)),
                            ..
                        }) => *l,
                        _ => crate::hir::LocalId(0),
                    },
                    subj_ty_id,
                );
                self.hir_push_stmt(crate::hir::HStmt::Let {
                    local: *local,
                    ty: *ty,
                    value: read,
                });
            }
            match &arm.body {
                SwitchArmBody::Expr(expr) => {
                    let t =
                        self.analyze_expression(expr, parent_function, &arm_scope, diagnostics)?;
                    let arm_hir = self.hir_take();
                    if is_expression {
                        match &arm_value_type {
                            None => arm_value_type = Some(t.clone()),
                            Some(prev) => {
                                self.compare_data_type(prev, &t, &empty_span(), diagnostics)?
                            }
                        }
                        // Desugar: assign the arm value to the shared result temp (allocated from the
                        // first arm's type), so the whole match reads back as one value.
                        if result_temp.is_none() {
                            result_temp = self.hir_alloc_local("__switch_result", &t);
                            result_ty_id = Some(self.type_ctx.lower(&t));
                        }
                        match result_temp {
                            Some(tmp) => self.hir_assign_local_id(tmp, arm_hir),
                            None => hir_ok = false,
                        }
                    } else {
                        self.hir_expr_stmt(arm_hir);
                    }
                }
                SwitchArmBody::Block(stmts) => {
                    if is_expression {
                        diagnostics.report_error(
                            "A block arm (`=> { ... }`) is only allowed when `switch` is used as a statement; use `=> expr` in expression position".to_string(),
                            arm.pattern.position(),
                        );
                    }
                    self.analyze_body(
                        stmts,
                        parent_function,
                        Some(&arm_scope),
                        false,
                        diagnostics,
                    )?;
                }
            }
            let body_hir = self.hir_close_block();

            match shape {
                HirArmShape::Default | HirArmShape::DefaultBind { .. } => hir_default = body_hir,
                HirArmShape::Const(label) => match self.hir_const_arm(Some(label), body_hir) {
                    Some(arm) => hir_arms.push(arm),
                    None => hir_ok = false,
                },
                HirArmShape::Variant {
                    def,
                    variant,
                    bindings,
                } => hir_arms.push(self.hir_variant_arm(def, variant, bindings, body_hir)),
                HirArmShape::Unsupported => hir_ok = false,
            }

            // Track the first irrefutable (catch-all) arm so later arms can be flagged unreachable.
            // (Exhaustiveness itself is decided from the arm patterns in `check_exhaustiveness`.)
            if arm.guard.is_none() && info.irrefutable {
                catch_all_index = Some(i);
            }
        }

        if is_expression {
            // Emit the desugared switch, then leave the result temp read as the match's value.
            match (result_temp, result_ty_id) {
                (Some(tmp), Some(ty)) if hir_ok => {
                    self.hir_switch(switch_scrutinee, hir_arms, hir_default, true);
                    self.hir_set_local_read(tmp, ty);
                }
                _ => {
                    self.hir_fail();
                    self.hir_none();
                }
            }
        } else {
            self.hir_switch(switch_scrutinee, hir_arms, hir_default, hir_ok);
        }

        self.check_exhaustiveness(
            &subject_base,
            &subject_type,
            &union_info,
            arms,
            subject.position(),
            diagnostics,
        );

        if is_expression {
            Ok(arm_value_type.unwrap_or(Type::Void))
        } else {
            Ok(Type::Void)
        }
    }

    /// Type-checks `pattern` against `expected`, introducing any bindings into `scope`.
    fn check_pattern(
        &mut self,
        pattern: &PatternNode,
        expected: &Type,
        scope: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<PatternInfo, SemanticError> {
        let expected_base = strip_nullable(&expected.get_type()).to_string();
        let union_info: Option<UnionInfo> = self.union_table.get(&expected_base).cloned();

        match pattern {
            PatternNode::Wildcard(_) => Ok(PatternInfo {
                irrefutable: true,
            }),
            PatternNode::Binding(name) => {
                // A bare identifier that names a unit variant of the matched union is a
                // unit-variant pattern; otherwise it binds the whole value.
                if let Some(info) = &union_info {
                    if let Some(v) = info.variant(&name.text) {
                        if v.fields.is_empty() {
                            return Ok(PatternInfo { irrefutable: false });
                        }
                    }
                }
                if let Err(e) = (*scope)
                    .borrow_mut()
                    .add_symbol(name.text.clone(), expected.clone())
                {
                    diagnostics.report_error(e.to_string(), Some(name.position));
                }
                Ok(PatternInfo {
                    irrefutable: true,
                })
            }
            PatternNode::Literal(lit) => {
                if !lit.is_unknown()
                    && !expected.is_unknown()
                    && !self.type_str_assignable(&expected_base, &lit.get_type())
                {
                    diagnostics.report_error(
                        format!(
                            "Pattern literal of type '{}' cannot match a value of type '{}'",
                            lit.get_type(),
                            expected_base
                        ),
                        lit.get_span(),
                    );
                }
                Ok(PatternInfo {
                    irrefutable: false,
                })
            }
            PatternNode::Variant(qualifier, variant, subs) => {
                let info = match &union_info {
                    Some(info) => info.clone(),
                    None => {
                        diagnostics.report_error(
                            format!(
                                "Variant pattern '{}' can only match a discriminated union, not '{}'",
                                variant.text, expected_base
                            ),
                            Some(variant.position),
                        );
                        // Still walk sub-patterns so their bindings/errors surface.
                        for sub in subs {
                            self.check_pattern(sub, &Type::Unknown, scope, diagnostics)?;
                        }
                        return Ok(PatternInfo {
                            irrefutable: false,
                        });
                    }
                };

                if let Some(q) = qualifier {
                    if q.text != expected_base {
                        diagnostics.report_error(
                            format!(
                                "Variant qualifier '{}' does not match the matched enum '{}'",
                                q.text, expected_base
                            ),
                            Some(q.position),
                        );
                    }
                }

                let var_info = match info.variant(&variant.text) {
                    Some(v) => v.clone(),
                    None => {
                        diagnostics.report_error(
                            format!("Enum '{}' has no variant '{}'", expected_base, variant.text),
                            Some(variant.position),
                        );
                        for sub in subs {
                            self.check_pattern(sub, &Type::Unknown, scope, diagnostics)?;
                        }
                        return Ok(PatternInfo {
                            irrefutable: false,
                        });
                    }
                };

                if subs.len() != var_info.fields.len() {
                    diagnostics.report_error(
                        format!(
                            "Variant '{}.{}' has {} field(s), but the pattern binds {}",
                            expected_base,
                            variant.text,
                            var_info.fields.len(),
                            subs.len()
                        ),
                        Some(variant.position),
                    );
                }

                // Recurse into each sub-pattern for its own type-checking / binding introduction.
                for (i, sub) in subs.iter().enumerate() {
                    let field_type = var_info
                        .fields
                        .get(i)
                        .map(|f| f.type_.clone())
                        .unwrap_or(Type::Unknown);
                    self.check_pattern(sub, &field_type, scope, diagnostics)?;
                }

                // A variant pattern is refutable on its own; whether the variant is fully covered
                // (across all arms, including nested sub-patterns) is decided in `check_exhaustiveness`.
                Ok(PatternInfo { irrefutable: false })
            }
        }
    }
    fn validate_variant_payload(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        field_types: &[Type],
        arg_types: &[Type],
        position: crate::text::text_span::TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        if arg_types.len() != field_types.len() {
            diagnostics.report_error(
                format!(
                    "Variant '{}.{}' expects {} argument(s), but {} were given",
                    enum_name,
                    variant_name,
                    field_types.len(),
                    arg_types.len()
                ),
                Some(position.clone()),
            );
        }

        let expected_strs: Vec<String> = field_types.iter().map(|t| t.get_type()).collect();
        let given_strs: Vec<String> = arg_types.iter().map(|t| t.get_type()).collect();

        self.validate_arguments(
            &format!("Variant '{}.{}'", enum_name, variant_name),
            &expected_strs,
            &given_strs,
            position,
            diagnostics,
        );
    }

    fn check_exhaustiveness(
        &self,
        subject_base: &str,
        subject_type: &Type,
        union_info: &Option<UnionInfo>,
        arms: &[SwitchArm<'a>],
        position: Option<crate::text::text_span::TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        // Only unguarded arms contribute to coverage (a guard may fail at runtime).
        let patterns: Vec<&PatternNode> = arms
            .iter()
            .filter(|a| a.guard.is_none())
            .map(|a| &a.pattern)
            .collect();

        // An irrefutable pattern (`_` or a whole-subject binding) covers everything.
        if patterns
            .iter()
            .any(|p| self.pattern_is_irrefutable(p, subject_type))
        {
            return;
        }

        if let Some(info) = union_info {
            // A variant is covered when a matching arm reaches it and (recursively) its payload
            // sub-patterns cover the field types — so `Wrap(A(n))` + `Wrap(B)` together cover `Wrap`.
            let missing: Vec<String> = info
                .variants
                .iter()
                .filter(|v| !self.variant_covered(&v.name, &v.fields, &patterns))
                .map(|v| v.name.clone())
                .collect();
            if !missing.is_empty() {
                diagnostics.report_error(
                    format!(
                        "Non-exhaustive switch on '{}': missing variant(s) {}. Add the missing arm(s) or a `_` arm",
                        subject_base,
                        missing.join(", ")
                    ),
                    position.clone(),
                );
            }
        } else if !subject_type.is_unknown() {
            diagnostics.report_error(
                format!(
                    "Non-exhaustive switch on '{}': add a `_` arm to cover all cases",
                    subject_base
                ),
                position.clone(),
            );
        }
    }

    /// True when the set of `patterns` (matched against a value of type `ty`) exhaustively covers
    /// every value of `ty`. Recurses into a union's variants and their single-field payloads, so a
    /// nested match like `Wrap(A(n))` + `Wrap(B)` is recognized as complete. Sound but conservative:
    /// a multi-field variant is only "covered" by an arm whose sub-patterns are all irrefutable
    /// (cartesian coverage across fields is not attempted), and an un-instantiated / non-union field
    /// type needs an irrefutable sub-pattern.
    fn patterns_exhaustive(&self, ty: &Type, patterns: &[&PatternNode]) -> bool {
        if patterns
            .iter()
            .any(|p| self.pattern_is_irrefutable(p, ty))
        {
            return true;
        }
        let base = strip_nullable(&ty.get_type()).to_string();
        let Some(info) = self.union_table.get(&base).cloned() else {
            return false;
        };
        info.variants
            .iter()
            .all(|v| self.variant_covered(&v.name, &v.fields, patterns))
    }

    /// True when `patterns` cover the union variant named `vname` (with `fields` payload).
    fn variant_covered(
        &self,
        vname: &str,
        fields: &[crate::semantics::union_table::UnionFieldInfo],
        patterns: &[&PatternNode],
    ) -> bool {
        match fields.len() {
            // Unit variant: covered by a matching unit pattern (`V` or `V()`).
            0 => patterns.iter().any(|p| Self::matches_unit_variant(p, vname)),
            // Single-field variant: covered when the sub-patterns at that field (gathered across all
            // arms matching this variant) recursively cover the field's type.
            1 => {
                let subs: Vec<&PatternNode> = patterns
                    .iter()
                    .filter_map(|p| Self::variant_sub(p, vname, 0))
                    .collect();
                !subs.is_empty() && self.patterns_exhaustive(&fields[0].type_, &subs)
            }
            // Multi-field variant: covered only by an arm binding every field irrefutably.
            _ => patterns
                .iter()
                .any(|p| self.variant_all_irrefutable(p, vname, fields)),
        }
    }

    /// True when `p` always matches a value of type `ty`: `_`, or a bare binding that names no unit
    /// variant of `ty`'s union (a unit-variant binding is refutable — it only matches that variant).
    fn pattern_is_irrefutable(&self, p: &PatternNode, ty: &Type) -> bool {
        match p {
            PatternNode::Wildcard(_) => true,
            PatternNode::Binding(name) => {
                let base = strip_nullable(&ty.get_type()).to_string();
                if let Some(info) = self.union_table.get(&base) {
                    if let Some(v) = info.variant(&name.text) {
                        if v.fields.is_empty() {
                            return false;
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// True when `p` is a unit-variant pattern for `vname` (`V` as a bare binding, or `V()`).
    fn matches_unit_variant(p: &PatternNode, vname: &str) -> bool {
        match p {
            PatternNode::Binding(name) => name.text == vname,
            PatternNode::Variant(_, name, subs) => name.text == vname && subs.is_empty(),
            _ => false,
        }
    }

    /// If `p` is a variant pattern for `vname`, returns its `i`-th sub-pattern.
    fn variant_sub<'p>(p: &'p PatternNode, vname: &str, i: usize) -> Option<&'p PatternNode> {
        match p {
            PatternNode::Variant(_, name, subs) if name.text == vname => subs.get(i),
            _ => None,
        }
    }

    /// True when `p` matches `vname` binding every field irrefutably (e.g. `Pair(a, b)` / `Pair(_, _)`).
    fn variant_all_irrefutable(
        &self,
        p: &PatternNode,
        vname: &str,
        fields: &[crate::semantics::union_table::UnionFieldInfo],
    ) -> bool {
        match p {
            PatternNode::Variant(_, name, subs) if name.text == vname && subs.len() == fields.len() => {
                subs.iter()
                    .zip(fields)
                    .all(|(s, f)| self.pattern_is_irrefutable(s, &f.type_))
            }
            _ => false,
        }
    }
}
