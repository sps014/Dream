//! Scalar replacement of aggregates. A struct allocated with the implicit zero-initializing default
//! constructor (`New { ctor: None }`) that never escapes — used only as the base of `obj.field`
//! loads and stores, never read whole, passed to a call, returned, stored elsewhere, indexed, or
//! reference-counted — has each of its fields promoted to a plain local. The allocation then becomes
//! dead (removed here) and the field locals feed the scalar pipeline (prop / GVN / DCE).
//!
//! The escape analysis is deliberately strict: the *only* statements allowed to mention the object
//! are its single `New` definition, field stores `obj.f = <op>`, and field loads `x = obj.f`. Any
//! other appearance (including an RC `Retain`/`Release`, which every heap object normally has) aborts
//! promotion, so the transform is always sound.

use super::licm::{stmt_reads, terminator_reads};
use super::MirPass;
use crate::mir::{
    Const, Local, LocalDecl, MirFunction, Operand, Place, Rvalue, Statement, Terminator,
};
use crate::types::{PrimTy, TyKind, TypeId, TypeInterner};
use std::collections::BTreeMap;

pub struct Sroa;

impl MirPass for Sroa {
    fn name(&self) -> &'static str {
        "sroa"
    }

    fn run(&self, func: &mut MirFunction, interner: &TypeInterner) -> bool {
        // Promote one object per call; the fixpoint reruns for the rest.
        let mut changed = false;
        for _ in 0..func.blocks.len().max(1) {
            if promote_one(func, interner) {
                changed = true;
            } else {
                break;
            }
        }
        changed
    }
}

fn promote_one(func: &mut MirFunction, interner: &TypeInterner) -> bool {
    // Find a candidate object: a single non-escaping default-constructed struct.
    let candidates = find_default_news(func, interner);
    for o in candidates {
        let Some(fields) = classify(func, interner, o) else {
            continue;
        };
        if fields.is_empty() {
            continue;
        }
        transform(func, interner, o, &fields);
        return true;
    }
    false
}

/// Locals assigned exactly once, by a `New { ctor: None }` of a non-value (heap) struct.
fn find_default_news(func: &MirFunction, interner: &TypeInterner) -> Vec<Local> {
    let mut def_counts: BTreeMap<Local, u32> = BTreeMap::new();
    let mut news: Vec<Local> = Vec::new();
    for block in &func.blocks {
        for stmt in &block.stmts {
            if let Statement::Assign(Place::Local(d), rv) = stmt {
                *def_counts.entry(*d).or_default() += 1;
                if let Rvalue::New { ctor: None, ty, .. } = rv {
                    if !interner.is_value_type(*ty) {
                        news.push(*d);
                    }
                }
            }
        }
    }
    news.into_iter()
        .filter(|o| def_counts.get(o).copied().unwrap_or(0) == 1)
        .collect()
}

/// Verifies `o` only appears in promotable field accesses and returns each accessed field's inferred
/// type, or `None` if any use disqualifies it.
fn classify(
    func: &MirFunction,
    interner: &TypeInterner,
    o: Local,
) -> Option<BTreeMap<usize, TypeId>> {
    let mut fields: BTreeMap<usize, TypeId> = BTreeMap::new();
    for block in &func.blocks {
        for stmt in &block.stmts {
            match stmt {
                // The single `New` definition itself: allowed, contributes no field.
                Statement::Assign(Place::Local(d), Rvalue::New { .. }) if *d == o => {}
                // Field store `o.f = <op>` (op must not itself mention `o`).
                Statement::Assign(Place::Field { base, field }, Rvalue::Use(op)) if *base == o => {
                    if operand_mentions(op, o) {
                        return None;
                    }
                    let ty = operand_ty(func, interner, op);
                    fields.entry(*field).or_insert(ty);
                }
                // Field load `x = o.f`.
                Statement::Assign(
                    Place::Local(x),
                    Rvalue::Use(Operand::Copy(Place::Field { base, field })),
                ) if *base == o => {
                    // The destination's declared type is the field's type (authoritative).
                    fields.insert(*field, func.local_ty(*x));
                }
                // Any other mention of `o` disqualifies promotion.
                _ => {
                    if stmt_mentions(stmt, o) {
                        return None;
                    }
                }
            }
        }
        if terminator_mentions(&block.terminator, o) {
            return None;
        }
    }
    Some(fields)
}

/// Replaces `o` with one promoted local per field: the `New` becomes zero-inits and every field
/// access is rewritten to the corresponding local.
fn transform(
    func: &mut MirFunction,
    interner: &TypeInterner,
    o: Local,
    fields: &BTreeMap<usize, TypeId>,
) {
    // Allocate a promoted local for each field.
    let mut promo: BTreeMap<usize, Local> = BTreeMap::new();
    for (&field, &ty) in fields {
        let l = Local(func.locals.len() as u32);
        func.locals.push(LocalDecl { ty, name: None });
        promo.insert(field, l);
    }

    // Rewrite field accesses; replace the `New` with zero-inits of every promoted field.
    for block in &mut func.blocks {
        let mut new_stmts: Vec<Statement> = Vec::with_capacity(block.stmts.len());
        for stmt in block.stmts.drain(..) {
            match stmt {
                Statement::Assign(Place::Local(d), Rvalue::New { .. }) if d == o => {
                    for (&field, &l) in &promo {
                        let zero = zero_for(interner, fields[&field]);
                        new_stmts.push(Statement::Assign(
                            Place::Local(l),
                            Rvalue::Use(Operand::Const(zero)),
                        ));
                    }
                }
                Statement::Assign(Place::Field { base, field }, rv) if base == o => {
                    new_stmts.push(Statement::Assign(Place::Local(promo[&field]), rv));
                }
                Statement::Assign(
                    Place::Local(x),
                    Rvalue::Use(Operand::Copy(Place::Field { base, field })),
                ) if base == o => {
                    new_stmts.push(Statement::Assign(
                        Place::Local(x),
                        Rvalue::Use(Operand::Copy(Place::Local(promo[&field]))),
                    ));
                }
                other => new_stmts.push(other),
            }
        }
        block.stmts = new_stmts;
    }
}

/// The zero value literal for a promoted field local of type `ty`.
fn zero_for(interner: &TypeInterner, ty: TypeId) -> Const {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Double) => Const::Float(0.0),
        TyKind::Prim(PrimTy::Float) => Const::F32(0.0),
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => Const::Long(0),
        _ => Const::Int(0),
    }
}

/// A representative interned type for an operand (used to type a field local from its stored value).
fn operand_ty(func: &MirFunction, interner: &TypeInterner, op: &Operand) -> TypeId {
    match op {
        Operand::Copy(Place::Local(l)) => func.local_ty(*l),
        Operand::Const(Const::Long(_)) => interner.long(),
        Operand::Const(Const::Float(_)) => interner.double(),
        Operand::Const(Const::F32(_)) => interner.float(),
        Operand::Const(Const::Bool(_)) => interner.bool(),
        Operand::Const(Const::Char(_)) => interner.char(),
        Operand::Const(Const::Str(_)) => interner.string(),
        _ => interner.int(),
    }
}

fn operand_mentions(op: &Operand, o: Local) -> bool {
    match op {
        Operand::Copy(Place::Local(l)) => *l == o,
        Operand::Copy(Place::Field { base, .. }) | Operand::Copy(Place::Index { base, .. }) => {
            *base == o
        }
        _ => false,
    }
}

fn stmt_mentions(stmt: &Statement, o: Local) -> bool {
    // Writes to `o` (as a place) plus any read of `o`.
    if let Statement::Assign(place, _) = stmt {
        if place_mentions(place, o) {
            return true;
        }
    }
    let mut hit = false;
    stmt_reads(stmt, &mut |l| {
        if l == o {
            hit = true;
        }
    });
    hit
}

fn terminator_mentions(t: &Terminator, o: Local) -> bool {
    let mut hit = false;
    terminator_reads(t, &mut |l| {
        if l == o {
            hit = true;
        }
    });
    if let Terminator::Await { dest: Some(d), .. } = t {
        if *d == o {
            hit = true;
        }
    }
    hit
}

fn place_mentions(place: &Place, o: Local) -> bool {
    match place {
        Place::Local(l) => *l == o,
        Place::Field { base, .. } | Place::Index { base, .. } => *base == o,
        Place::Global(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{DefId, Rvalue};

    #[test]
    fn promotes_non_escaping_struct() {
        // o = new S(); o.0 = 7; x = o.0; return x;  ->  o and its field become a local.
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let o = b.new_temp(i.int());
        let x = b.new_temp(i.int());
        b.assign(
            Place::Local(o),
            Rvalue::New {
                def: DefId(0),
                ty: i.int(),
                ctor: None,
                args: vec![],
            },
        );
        b.assign(
            Place::Field { base: o, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(7))),
        );
        b.assign(
            Place::Local(x),
            Rvalue::Use(Operand::Copy(Place::Field { base: o, field: 0 })),
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(x)))));
        let mut func = b.finish();

        assert!(
            Sroa.run(&mut func, &i),
            "non-escaping struct should be promoted"
        );
        let has_new = func
            .blocks
            .iter()
            .flat_map(|bb| &bb.stmts)
            .any(|s| matches!(s, Statement::Assign(_, Rvalue::New { .. })));
        assert!(!has_new, "the allocation should be gone");
        let has_field = func.blocks.iter().flat_map(|bb| &bb.stmts).any(|s| {
            matches!(s, Statement::Assign(Place::Field { .. }, _))
                || matches!(
                    s,
                    Statement::Assign(_, Rvalue::Use(Operand::Copy(Place::Field { .. })))
                )
        });
        assert!(!has_field, "field accesses should be rewritten to locals");
    }

    #[test]
    fn does_not_promote_escaping_struct() {
        // o escapes by being returned whole.
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let o = b.new_temp(i.int());
        b.assign(
            Place::Local(o),
            Rvalue::New {
                def: DefId(0),
                ty: i.int(),
                ctor: None,
                args: vec![],
            },
        );
        b.assign(
            Place::Field { base: o, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(7))),
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(o)))));
        let mut func = b.finish();
        assert!(
            !Sroa.run(&mut func, &i),
            "escaping struct must not be promoted"
        );
    }
}
