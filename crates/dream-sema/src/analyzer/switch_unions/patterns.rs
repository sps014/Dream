//! Pattern classification/compilation shared by both switch-lowering paths in [`super::lowering`]:
//! [`Analyzer::hir_switch_pattern`] classifies a pattern into the [`super::HirArmShape`] the
//! `Switch`-emitting fast path needs, while [`Analyzer::compile_pattern`] compiles a pattern into
//! explicit boolean test conditions for the general if-chain fallback.
//! [`Analyzer::expand_switch_arms_for_fast_path`] turns or-patterns and small literal ranges into
//! flat multi-key arms; [`Analyzer::pattern_switch_needs_full_chain`] /
//! [`Analyzer::pattern_switch_needs_residual`] decide which path remains.

use super::*;
use crate::union_table::UnionInfo;
use dream_syntax::nodes::{PatternNode, SwitchArm, Type};
use dream_syntax::token::syntax_token::SyntaxToken;
use dream_syntax::token::token_kind::TokenKind;

impl<'a> Analyzer<'a> {
    /// Classifies a switch pattern for HIR statement-`switch` lowering, allocating HIR locals for any
    /// variant-payload bindings *before* the arm body is lowered so the body can resolve them.
    pub(super) fn hir_switch_pattern(
        &mut self,
        pattern: &PatternNode,
        union_info: &Option<UnionInfo>,
        union_def: Option<dream_types::DefId>,
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
            // Range/Or are expanded into flat Const/Variant arms before the Switch path runs; if
            // one still reaches here it is an ICE-class routing gap.
            PatternNode::Range(..) | PatternNode::Or(..) => HirArmShape::Unsupported,
        }
    }

    /// Cap on inclusive literal range expansion onto multi-key `Switch` arms (`90..100` → 11 arms).
    /// Larger ranges stay on the if-chain path.
    const RANGE_EXPAND_MAX: i64 = 256;

    /// Expands top-level or-patterns and small int/char literal ranges into flat arms so they can
    /// lower through `HStmt::Switch` (multi-key arms sharing a cloned body, like C-style multi-label
    /// cases). Unexpandable ranges and nested/guarded shapes are left unchanged for if-chain routing.
    pub(super) fn expand_switch_arms_for_fast_path<'b>(
        arms: &[SwitchArm<'b>],
    ) -> Vec<SwitchArm<'b>> {
        let mut out = Vec::with_capacity(arms.len());
        for arm in arms {
            match &arm.pattern {
                PatternNode::Or(alts)
                    if !alts.iter().any(Self::pattern_needs_chain) =>
                {
                    for alt in alts {
                        out.push(SwitchArm {
                            pattern: alt.clone(),
                            guard: arm.guard.clone(),
                            body: arm.body.clone(),
                        });
                    }
                }
                PatternNode::Range(lo, hi) => {
                    if let Some(lits) = Self::expand_range_literals(lo, hi) {
                        for lit in lits {
                            out.push(SwitchArm {
                                pattern: PatternNode::Literal(lit),
                                guard: arm.guard.clone(),
                                body: arm.body.clone(),
                            });
                        }
                    } else {
                        out.push(arm.clone());
                    }
                }
                _ => out.push(arm.clone()),
            }
        }
        out
    }

    /// Inclusive int/char literal range → one `Type` literal per value, or `None` when the span is
    /// too large, inverted, or not an int/char literal pair.
    fn expand_range_literals(lo: &Type, hi: &Type) -> Option<Vec<Type>> {
        match (lo, hi) {
            (Type::Integer(a), Type::Integer(b)) => {
                let lo_v = a.text.parse::<i64>().ok()?;
                let hi_v = b.text.parse::<i64>().ok()?;
                if hi_v < lo_v {
                    return None;
                }
                let span = hi_v.checked_sub(lo_v)?.checked_add(1)?;
                if span > Self::RANGE_EXPAND_MAX {
                    return None;
                }
                Some(
                    (lo_v..=hi_v)
                        .map(|v| {
                            Type::Integer(SyntaxToken::new(
                                TokenKind::NumberToken,
                                a.position,
                                v.to_string(),
                            ))
                        })
                        .collect(),
                )
            }
            (Type::Char(a), Type::Char(b)) => {
                let lo_v = a.text.parse::<u32>().ok().and_then(char::from_u32)?;
                let hi_v = b.text.parse::<u32>().ok().and_then(char::from_u32)?;
                if hi_v < lo_v {
                    return None;
                }
                let span = (u32::from(hi_v) - u32::from(lo_v) + 1) as i64;
                if span > Self::RANGE_EXPAND_MAX {
                    return None;
                }
                Some(
                    (lo_v..=hi_v)
                        .map(|c| {
                            Type::Char(SyntaxToken::new(
                                TokenKind::CharToken,
                                a.position,
                                u32::from(c).to_string(),
                            ))
                        })
                        .collect(),
                )
            }
            _ => None,
        }
    }

    /// True when `arms` cannot participate in any outer `Switch`/`br_table` at all (an unexpanded
    /// range, or an or-pattern whose alternatives themselves lack an outer key). Nested/literal
    /// sub-patterns and guards are handled by the hybrid outer-Switch + residual path instead.
    pub(super) fn pattern_switch_needs_full_chain(arms: &[SwitchArm]) -> bool {
        arms.iter().any(|a| Self::pattern_lacks_outer_key(&a.pattern))
    }

    /// True when `arms` need residual if-chain work inside Switch arms (guards or nested
    /// sub-patterns) but still have representable outer keys.
    pub(super) fn pattern_switch_needs_residual(arms: &[SwitchArm]) -> bool {
        arms.iter()
            .any(|a| a.guard.is_some() || Self::pattern_needs_chain(&a.pattern))
    }

    /// True when the pattern has no Switch-representable outer key even after expansion.
    fn pattern_lacks_outer_key(p: &PatternNode) -> bool {
        match p {
            PatternNode::Range(..) => true,
            PatternNode::Or(alts) => alts.iter().any(Self::pattern_lacks_outer_key),
            _ => false,
        }
    }

    /// True for a pattern that the flat `Switch` arm shape cannot represent after expansion:
    /// a variant with a nested/literal sub-pattern, an unexpanded range, or an unexpanded or-pattern
    /// (or whose alternatives themselves need the chain).
    fn pattern_needs_chain(p: &PatternNode) -> bool {
        match p {
            PatternNode::Variant(_, _, subs) => subs
                .iter()
                .any(|s| !matches!(s, PatternNode::Binding(_) | PatternNode::Wildcard(_))),
            PatternNode::Range(..) => true,
            PatternNode::Or(alts) => alts.iter().any(Self::pattern_needs_chain),
            _ => false,
        }
    }

    /// Recursively compiles `pattern` (matched against value `value` of type `value_type`) into a set
    /// of boolean test conditions plus named payload bindings. Returns `None` if the pattern isn't
    /// representable (so the caller drops the function). All field reads are inlined into the returned
    /// expressions, so the conditions/bindings are self-contained (no reliance on prior bindings).
    #[allow(clippy::type_complexity)]
    pub(super) fn compile_pattern(
        &mut self,
        value: &dream_hir::HExpr,
        value_type: &Type,
        pattern: &PatternNode,
    ) -> Option<(
        Vec<dream_hir::HExpr>,
        Vec<(String, Type, dream_hir::HExpr)>,
    )> {
        use dream_hir::{BinOp, HExpr, HExprKind};
        let base = value_type.get_type();
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
                let union_ty_id = lowered;
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
            PatternNode::Range(lo, hi) => {
                self.hir_set_literal(lo);
                let lo_e = self.hir_take()?;
                self.hir_set_literal(hi);
                let hi_e = self.hir_take()?;
                let ge = self.hx_bin(BinOp::Ge, value.clone(), lo_e);
                let le = self.hx_bin(BinOp::Le, value.clone(), hi_e);
                Some((vec![self.hx_bin(BinOp::And, ge, le)], vec![]))
            }
            PatternNode::Or(alts) => {
                // Alternatives are validated binding-free in `check_pattern`, so only the boolean
                // test conditions matter here: each alternative's conditions are ANDed together
                // (it must fully match), then the alternatives themselves are ORed.
                let mut combined: Option<HExpr> = None;
                for alt in alts {
                    let (conds, _binds) = self.compile_pattern(value, value_type, alt)?;
                    let alt_cond = conds
                        .into_iter()
                        .reduce(|a, b| self.hx_bin(BinOp::And, a, b))
                        .unwrap_or_else(|| self.hx_bool(true));
                    combined = Some(match combined {
                        Some(acc) => self.hx_bin(BinOp::Or, acc, alt_cond),
                        None => alt_cond,
                    });
                }
                Some((vec![combined?], vec![]))
            }
        }
    }
}
