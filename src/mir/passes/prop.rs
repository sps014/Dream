//! Intra-block copy and constant propagation. Within a single basic block, a local that is assigned
//! a constant or a copy of another local has its later reads replaced by the source value. The
//! analysis is reset at block boundaries (no cross-block dataflow), which keeps it simple and sound
//! without SSA phi handling.

use super::MirPass;
use crate::mir::{Local, MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use crate::types::TypeInterner;
use std::collections::HashMap;

pub struct CopyConstProp;

impl MirPass for CopyConstProp {
    fn name(&self) -> &'static str {
        "copy-const-prop"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = false;
        for block in &mut func.blocks {
            let mut known: HashMap<Local, Operand> = HashMap::new();
            for stmt in &mut block.stmts {
                changed |= subst_stmt_reads(stmt, &known);
                update_known(stmt, &mut known);
            }
            changed |= subst_terminator_reads(&mut block.terminator, &known);
        }
        changed
    }
}

/// Resolves an operand through the known-value map (chasing copies transitively).
fn resolve(op: &Operand, known: &HashMap<Local, Operand>) -> Option<Operand> {
    if let Operand::Copy(Place::Local(l)) = op {
        if let Some(v) = known.get(l) {
            // Chase further in case `v` is itself a propagated copy.
            return Some(resolve(v, known).unwrap_or_else(|| v.clone()));
        }
    }
    None
}

fn subst_operand(op: &mut Operand, known: &HashMap<Local, Operand>) -> bool {
    if let Some(v) = resolve(op, known) {
        *op = v;
        return true;
    }
    false
}

fn subst_place_reads(place: &mut Place, known: &HashMap<Local, Operand>) -> bool {
    // Only the index operand of an `Index` place is a *read*; the base local is a destination/base.
    if let Place::Index { index, .. } = place {
        return subst_operand(index, known);
    }
    false
}

fn subst_stmt_reads(stmt: &mut Statement, known: &HashMap<Local, Operand>) -> bool {
    match stmt {
        Statement::Assign(place, rvalue) => {
            let mut c = subst_place_reads(place, known);
            c |= subst_rvalue_reads(rvalue, known);
            c
        }
        Statement::Retain(o) | Statement::Release(o) => subst_operand(o, known),
        Statement::Call { args, .. } => args
            .iter_mut()
            .fold(false, |c, a| c | subst_operand(a, known)),
        Statement::InterfaceCall { receiver, args, .. } => {
            let mut c = subst_operand(receiver, known);
            for a in args {
                c |= subst_operand(a, known);
            }
            c
        }
        Statement::Print { arg, .. } => subst_operand(arg, known),
        Statement::Nop => false,
    }
}

fn subst_rvalue_reads(rvalue: &mut Rvalue, known: &HashMap<Local, Operand>) -> bool {
    match rvalue {
        Rvalue::Use(o)
        | Rvalue::ArrayLen(o)
        | Rvalue::StrLen(o)
        | Rvalue::Cast(o, _, _)
        | Rvalue::IsType(o, _)
        | Rvalue::Discriminant(o)
        | Rvalue::HashCode(o)
        | Rvalue::ToString(o)
        | Rvalue::UnionField { base: o, .. } => subst_operand(o, known),
        Rvalue::Binary(_, a, b) | Rvalue::CharAt(a, b) | Rvalue::Concat(a, b) => {
            subst_operand(a, known) | subst_operand(b, known)
        }
        Rvalue::EnumName { value, .. } => subst_operand(value, known),
        Rvalue::ArrayNew { len, .. } => subst_operand(len, known),
        Rvalue::Unary(_, a) => subst_operand(a, known),
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::UnionNew { args, .. }
        | Rvalue::ArrayLit { elems: args, .. } => args
            .iter_mut()
            .fold(false, |c, a| c | subst_operand(a, known)),
        Rvalue::IndirectCall { target, args } => {
            let mut c = subst_operand(target, known);
            for a in args {
                c |= subst_operand(a, known);
            }
            c
        }
        Rvalue::InterfaceCall { receiver, args, .. } => {
            let mut c = subst_operand(receiver, known);
            for a in args {
                c |= subst_operand(a, known);
            }
            c
        }
        Rvalue::JsCall {
            target,
            method,
            args,
            ..
        } => {
            let mut c = subst_operand(target, known);
            if let Some(m) = method {
                c |= subst_operand(m, known);
            }
            for (a, _) in args {
                c |= subst_operand(a, known);
            }
            c
        }
        Rvalue::FuncRef(_) => false,
    }
}

fn subst_terminator_reads(t: &mut Terminator, known: &HashMap<Local, Operand>) -> bool {
    match t {
        Terminator::If { cond, .. } => subst_operand(cond, known),
        Terminator::Switch { value, .. } => subst_operand(value, known),
        Terminator::Return(Some(o)) => subst_operand(o, known),
        Terminator::AsyncComplete(Some(o)) => subst_operand(o, known),
        _ => false,
    }
}

/// Updates the known-value map after a statement executes.
fn update_known(stmt: &Statement, known: &mut HashMap<Local, Operand>) {
    if let Statement::Assign(Place::Local(dest), rvalue) = stmt {
        // The destination's old value is gone, and any entry that *copied* it is now stale.
        invalidate(*dest, known);
        if let Rvalue::Use(op @ (Operand::Const(_) | Operand::Copy(Place::Local(_)))) = rvalue {
            known.insert(*dest, op.clone());
        }
    } else if let Statement::Assign(_, _) = stmt {
        // Stores through field/index/global may alias; be conservative and keep only consts.
        known.retain(|_, v| matches!(v, Operand::Const(_)));
    }
    // Calls may mutate through references; constants stay valid, copies of locals are kept (locals
    // are not aliased by value here).
}

fn invalidate(dest: Local, known: &mut HashMap<Local, Operand>) {
    known.remove(&dest);
    known.retain(|_, v| !matches!(v, Operand::Copy(Place::Local(l)) if *l == dest));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Const, Operand, Place, Rvalue, Terminator};

    #[test]
    fn propagates_const_into_return() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let x = b.new_temp(i.int());
        b.assign(Place::Local(x), Rvalue::Use(Operand::Const(Const::Int(7))));
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(x)))));
        let mut func = b.finish();
        assert!(CopyConstProp.run(&mut func, &i));
        match &func.blocks[0].terminator {
            Terminator::Return(Some(Operand::Const(Const::Int(v)))) => assert_eq!(*v, 7),
            other => panic!("expected propagated const, got {:?}", other),
        }
    }
}
