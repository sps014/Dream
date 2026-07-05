//! Pattern classification/compilation shared by both switch-lowering paths in [`super::lowering`]:
//! [`Analyzer::hir_switch_pattern`] classifies a pattern into the [`super::HirArmShape`] the
//! `Switch`-emitting fast path needs, while [`Analyzer::compile_pattern`] compiles a pattern into
//! explicit boolean test conditions for the general if-chain fallback. [`Analyzer::pattern_switch_needs_chain`]
//! decides which path a given switch's arms require.

use super::*;
use crate::semantics::union_table::UnionInfo;
use crate::syntax::nodes::types::strip_nullable;
use crate::syntax::nodes::{PatternNode, SwitchArm, Type};

impl<'a> Analyzer<'a> {
    /// Classifies a switch pattern for HIR statement-`switch` lowering, allocating HIR locals for any
    /// variant-payload bindings *before* the arm body is lowered so the body can resolve them.
    pub(super) fn hir_switch_pattern(
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
    pub(super) fn pattern_switch_needs_chain(arms: &[SwitchArm]) -> bool {
        arms.iter()
            .any(|a| a.guard.is_some() || Self::pattern_is_nested(&a.pattern))
    }

    /// True for a variant pattern with at least one sub-pattern that isn't a flat binding/wildcard.
    fn pattern_is_nested(p: &PatternNode) -> bool {
        matches!(p, PatternNode::Variant(_, _, subs)
            if subs.iter().any(|s| !matches!(s, PatternNode::Binding(_) | PatternNode::Wildcard(_))))
    }

    /// Recursively compiles `pattern` (matched against value `value` of type `value_type`) into a set
    /// of boolean test conditions plus named payload bindings. Returns `None` if the pattern isn't
    /// representable (so the caller drops the function). All field reads are inlined into the returned
    /// expressions, so the conditions/bindings are self-contained (no reliance on prior bindings).
    #[allow(clippy::type_complexity)]
    pub(super) fn compile_pattern(
        &mut self,
        value: &crate::hir::HExpr,
        value_type: &Type,
        pattern: &PatternNode,
    ) -> Option<(
        Vec<crate::hir::HExpr>,
        Vec<(String, Type, crate::hir::HExpr)>,
    )> {
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
                Some((
                    vec![],
                    vec![(name.text.clone(), value_type.clone(), value.clone())],
                ))
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
}
