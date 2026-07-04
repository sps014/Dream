//! Reference-counting passes.
//!
//! [`RcInsertion`] (run as a MIR pass) makes ownership explicit under a single invariant: **every
//! non-parameter reference local owns exactly one reference count.** It is upheld by three rules:
//!
//! 1. *Local assignment* — when a borrowed reference is copied into a reference local it inserts a
//!    `Retain` (the local becomes a new owner); before a reference local is overwritten it inserts a
//!    `Release` of the previous value (releasing the zero-initialized null on first assignment is a
//!    runtime no-op). Owned producers (call results, `new`, array literals) already carry their
//!    `+1`, so they are not retained.
//! 2. *Container stores* (handled in the emitter, not here) retain a borrowed reference written into
//!    a struct field / array element / union payload, so the container owns its own count and the
//!    source local keeps its own.
//! 3. *Scope exit* — at every `Return`, release each non-parameter reference local. The returned
//!    value is excluded: an owned local transfers its `+1` to the caller, and a borrowed return
//!    (parameter, field, or element read) is spilled to a fresh temporary and retained so it
//!    survives the releases and hands the caller a `+1`.
//!
//! Parameters are borrowed (the caller owns them), so they are never released at scope exit and call
//! arguments are not retained — a self-consistent ABI: callee-owns-none-of-its-params,
//! caller-owns-the-result.
//!
//! [`RcElision`] cancels redundant adjacent `Retain`/`Release` pairs on the same operand,
//! the payoff once propagation/inlining bring a retain and its matching release together.

use super::MirPass;
use crate::mir::{Local, LocalDecl, MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use crate::types::TypeInterner;
use std::collections::HashSet;

pub struct RcInsertion;

impl MirPass for RcInsertion {
    fn name(&self) -> &'static str {
        "rc-insertion"
    }

    fn run(&self, func: &mut MirFunction, interner: &TypeInterner) -> bool {
        let local_is_ref: Vec<bool> = func
            .locals
            .iter()
            .map(|d| interner.is_reference(d.ty))
            .collect();
        let params: HashSet<u32> = func.params.iter().map(|p| p.0).collect();
        let is_owned_ref =
            |l: u32| local_is_ref.get(l as usize).copied().unwrap_or(false) && !params.contains(&l);
        let mut changed = false;

        // Rule 1: local-assignment RC (release previous occupant, retain borrowed copies). When the
        // new value depends on the *old* one (e.g. `list = Cons(i, list)`), the old value must be
        // released *after* the rvalue is evaluated (the rvalue's container store retains it), not
        // before — otherwise a `+0` old value is freed and then reused mid-evaluation. Such cases
        // stash the old pointer in a synthetic temp and release it after the store.
        let local_types: Vec<crate::types::TypeId> = func.locals.iter().map(|d| d.ty).collect();
        let mut extra_locals: Vec<LocalDecl> = Vec::new();
        let temp_base = func.locals.len() as u32;
        for block in &mut func.blocks {
            let mut out: Vec<Statement> = Vec::with_capacity(block.stmts.len());
            for stmt in block.stmts.drain(..) {
                let ref_dest = match &stmt {
                    Statement::Assign(Place::Local(dest), rvalue) if is_owned_ref(dest.0) => {
                        Some((
                            *dest,
                            is_borrowed_copy(rvalue, interner),
                            rvalue_reads_local(rvalue, dest.0),
                        ))
                    }
                    _ => None,
                };
                match ref_dest {
                    Some((dest, retain, true)) => {
                        assert!(
                            is_owned_ref(dest.0),
                            "RC insertion on non-owned reference local"
                        );
                        // Old value is read by the rvalue: save it, evaluate, then release it.
                        let tmp = Local(temp_base + extra_locals.len() as u32);
                        extra_locals.push(LocalDecl {
                            ty: local_types[dest.0 as usize],
                            name: None,
                        });
                        out.push(Statement::Assign(
                            Place::Local(tmp),
                            Rvalue::Use(Operand::Copy(Place::Local(dest))),
                        ));
                        out.push(stmt);
                        if retain {
                            out.push(Statement::Retain(Operand::Copy(Place::Local(dest))));
                        }
                        out.push(Statement::Release(Operand::Copy(Place::Local(tmp))));
                        changed = true;
                    }
                    Some((dest, retain, false)) => {
                        out.push(Statement::Release(Operand::Copy(Place::Local(dest))));
                        out.push(stmt);
                        if retain {
                            out.push(Statement::Retain(Operand::Copy(Place::Local(dest))));
                        }
                        changed = true;
                    }
                    None => out.push(stmt),
                }
            }
            block.stmts = out;
        }
        // Synthetic old-value temps are pure aliases used only for the deferred release; they must not
        // be released again at scope exit (they are beyond `local_is_ref`, so `is_owned_ref` already
        // excludes them from Rule 3 below).
        func.locals.extend(extra_locals);

        // Rule 3: scope-exit release at every `Return`.
        let owned_locals: Vec<u32> = (0..func.locals.len() as u32)
            .filter(|i| is_owned_ref(*i))
            .collect();
        let ret_is_ref = interner.is_reference(func.ret);
        let mut spills: Vec<LocalDecl> = Vec::new();
        let next_local = func.locals.len() as u32;
        for block in &mut func.blocks {
            let Terminator::Return(ret) = &block.terminator else {
                continue;
            };
            // Decide whether the return value transfers (owned local) or must be spilled + retained.
            let (skip, spill_from): (Option<u32>, Option<Operand>) = match ret {
                Some(Operand::Copy(Place::Local(l))) if is_owned_ref(l.0) => (Some(l.0), None),
                Some(op) if ret_is_ref => (None, Some(op.clone())),
                _ => (None, None),
            };
            let skip = if let Some(op) = spill_from {
                let temp = Local(next_local + spills.len() as u32);
                spills.push(LocalDecl {
                    ty: func.ret,
                    name: None,
                });
                block
                    .stmts
                    .push(Statement::Assign(Place::Local(temp), Rvalue::Use(op)));
                block
                    .stmts
                    .push(Statement::Retain(Operand::Copy(Place::Local(temp))));
                block.terminator = Terminator::Return(Some(Operand::Copy(Place::Local(temp))));
                changed = true;
                Some(temp.0)
            } else {
                skip
            };
            for &i in &owned_locals {
                if Some(i) == skip {
                    continue;
                }
                block
                    .stmts
                    .push(Statement::Release(Operand::Copy(Place::Local(Local(i)))));
                changed = true;
            }
        }
        func.locals.extend(spills);
        changed
    }
}

/// True if the rvalue is a *borrow* that must be retained when bound to an owning local, as opposed
/// to a freshly-owned value (call/new/array literal) that already carries its `+1`. Two cases: a
/// copy of an existing reference place, and an interned string literal (which lives at a baseline
/// refcount of 1 in the string pool, so a binding that will later be released must first retain it
/// to keep the shared literal alive).
/// True if `local` is read anywhere in `rvalue` (as a plain operand or through a field/index base).
/// Used to detect self-referential reassignments (`x = f(x)`) whose old value must outlive the
/// rvalue's evaluation.
fn rvalue_reads_local(rvalue: &Rvalue, local: u32) -> bool {
    let mut hit = false;
    let mut check = |op: &Operand| {
        if let Operand::Copy(place) = op {
            let base = match place {
                Place::Local(l) => Some(l.0),
                Place::Field { base, .. } => Some(base.0),
                Place::Index { base, .. } => Some(base.0),
                Place::Global(_) => None,
            };
            if base == Some(local) {
                hit = true;
            }
            if let Place::Index { index, .. } = place {
                if let Operand::Copy(Place::Local(l)) = index.as_ref() {
                    if l.0 == local {
                        hit = true;
                    }
                }
            }
        }
    };
    match rvalue {
        Rvalue::Select {
            cond,
            then_val,
            else_val,
        } => {
            check(cond);
            check(then_val);
            check(else_val);
        }
        Rvalue::Use(o)
        | Rvalue::Unary(_, o)
        | Rvalue::ArrayLen(o)
        | Rvalue::StrLen(o)
        | Rvalue::Cast(o, _, _)
        | Rvalue::IsType(o, _)
        | Rvalue::Discriminant(o)
        | Rvalue::HashCode(o)
        | Rvalue::ToString(o)
        | Rvalue::UnionField { base: o, .. } => check(o),
        Rvalue::Binary(_, a, b) | Rvalue::CharAt(a, b) | Rvalue::Concat(a, b) => {
            check(a);
            check(b);
        }
        Rvalue::EnumName { value, .. } => check(value),
        Rvalue::ArrayNew { len, .. } => check(len),
        Rvalue::ToBytes { value: o, .. } | Rvalue::FromBytes { bytes: o, .. } => check(o),
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::UnionNew { args, .. }
        | Rvalue::ArrayLit { elems: args, .. } => args.iter().for_each(&mut check),
        Rvalue::IndirectCall { target, args } => {
            check(target);
            args.iter().for_each(&mut check);
        }
        Rvalue::InterfaceCall { receiver, args, .. } => {
            check(receiver);
            args.iter().for_each(&mut check);
        }
        Rvalue::JsCall {
            target,
            method,
            args,
            ..
        } => {
            check(target);
            if let Some(m) = method {
                check(m);
            }
            args.iter().for_each(|(a, _)| check(a));
        }
        Rvalue::FuncRef(_) => {}
    }
    hit
}

fn is_borrowed_copy(rvalue: &Rvalue, interner: &TypeInterner) -> bool {
    match rvalue {
        Rvalue::Use(Operand::Copy(_))
        | Rvalue::Use(Operand::Const(crate::mir::Const::Str(_)))
        // A union payload field read is a borrow of the union's own reference (like a struct
        // field read), so a reference binding must retain it to balance its scope-exit release.
        | Rvalue::UnionField { .. } => true,
        // A reference-to-reference cast (e.g. `(Animal)cat`, an interface up/downcast) is pointer
        // identity: the destination local aliases the source's reference, so it is a borrow and must
        // be retained to balance its scope-exit release — otherwise the shared pointer is
        // double-freed. Reference→primitive unboxes and numeric casts produce fresh values.
        Rvalue::Cast(Operand::Copy(_), from, to) => {
            interner.is_reference(*from) && interner.is_reference(*to)
        }
        _ => false,
    }
}

pub struct RcElision;

impl MirPass for RcElision {
    fn name(&self) -> &'static str {
        "rc-elision"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = false;
        for block in &mut func.blocks {
            let mut i = 0;
            while i + 1 < block.stmts.len() {
                let cancel = matches!(
                    (&block.stmts[i], &block.stmts[i + 1]),
                    (Statement::Retain(a), Statement::Release(b)) if operand_eq(a, b)
                );
                if cancel {
                    block.stmts.drain(i..i + 2);
                    changed = true;
                    // Re-examine the position in case another pair is now adjacent.
                    i = i.saturating_sub(1);
                } else {
                    i += 1;
                }
            }
        }
        changed
    }
}

fn operand_eq(a: &Operand, b: &Operand) -> bool {
    match (a, b) {
        (Operand::Copy(Place::Local(x)), Operand::Copy(Place::Local(y))) => x == y,
        (Operand::Copy(Place::Global(x)), Operand::Copy(Place::Global(y))) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Local, Operand, Place, Terminator};

    #[test]
    fn elides_adjacent_retain_release() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.void());
        b.push(Statement::Retain(Operand::Copy(Place::Local(Local(0)))));
        b.push(Statement::Release(Operand::Copy(Place::Local(Local(0)))));
        b.terminate(Terminator::Return(None));
        let mut func = b.finish();
        assert!(RcElision.run(&mut func, &i));
        assert!(func.blocks[0].stmts.is_empty());
    }

    #[test]
    fn inserts_retain_on_borrowed_copy() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.void());
        let s = b.new_local(i.string(), Some("s".into()));
        let t = b.new_local(i.string(), Some("t".into()));
        // t = s   (borrowed copy of a reference)
        b.assign(Place::Local(t), Rvalue::Use(Operand::Copy(Place::Local(s))));
        b.terminate(Terminator::Return(None));
        let mut func = b.finish();
        assert!(RcInsertion.run(&mut func, &i));
        // Rule 1 gives `release t (old); assign; retain t`; Rule 3 then releases both owned reference
        // locals (`s`, `t`) at the `Return`.
        let kinds: Vec<&str> = func.blocks[0]
            .stmts
            .iter()
            .map(|s| match s {
                Statement::Release(_) => "release",
                Statement::Assign(..) => "assign",
                Statement::Retain(_) => "retain",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["release", "assign", "retain", "release", "release"]
        );
    }

    #[test]
    fn returned_owned_local_is_not_released() {
        // `fun f(): string { let s = "x"; return s; }` — `s` owns its `+1` and transfers it to the
        // caller, so no `Release` of `s` is inserted at the return.
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.string());
        let s = b.new_local(i.string(), Some("s".into()));
        b.assign(
            Place::Local(s),
            Rvalue::Use(Operand::Const(crate::mir::Const::Str("x".into()))),
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(s)))));
        let mut func = b.finish();
        RcInsertion.run(&mut func, &i);
        let releases = func.blocks[0]
            .stmts
            .iter()
            .filter(|s| matches!(s, Statement::Release(_)))
            .count();
        // Only the release-before-overwrite of the (null) previous value; none at scope exit for `s`.
        assert_eq!(releases, 1);
        assert!(matches!(
            func.blocks[0].terminator,
            Terminator::Return(Some(Operand::Copy(Place::Local(l)))) if l == s
        ));
    }
}
