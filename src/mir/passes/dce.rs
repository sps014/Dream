//! Dead-code elimination: drops blocks unreachable from entry, removes assignments to locals that
//! are never read (when the RHS is side-effect free), and strips `Nop`s. Runs to a fixpoint via the
//! pass manager (one removal can expose another).

use super::MirPass;
use crate::mir::{BlockId, Local, MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use crate::types::TypeInterner;
use std::collections::HashSet;

pub struct Dce;

impl MirPass for Dce {
    fn name(&self) -> &'static str {
        "dce"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = drop_unreachable_blocks(func);
        changed |= remove_dead_assignments(func);
        changed
    }
}

fn drop_unreachable_blocks(func: &mut MirFunction) -> bool {
    let reachable = reachable_blocks(func);
    let mut changed = false;
    for (i, block) in func.blocks.iter_mut().enumerate() {
        let already_empty =
            block.stmts.is_empty() && matches!(block.terminator, Terminator::Unreachable);
        if !reachable.contains(&BlockId(i as u32)) && !already_empty {
            block.stmts.clear();
            block.terminator = Terminator::Unreachable;
            changed = true;
        }
    }
    changed
}

fn reachable_blocks(func: &MirFunction) -> HashSet<BlockId> {
    let mut seen = HashSet::new();
    let mut stack = vec![func.entry];
    while let Some(b) = stack.pop() {
        if !seen.insert(b) {
            continue;
        }
        for s in func.block(b).terminator.successors() {
            stack.push(s);
        }
    }
    seen
}

fn remove_dead_assignments(func: &mut MirFunction) -> bool {
    let read = collect_read_locals(func);
    let mut changed = false;
    for block in &mut func.blocks {
        let before = block.stmts.len();
        block.stmts.retain(|stmt| match stmt {
            Statement::Nop => false,
            // A self-copy `x = x` is always dead (produced by CSE/copy-prop collapsing a redundant
            // expression into its own source).
            Statement::Assign(Place::Local(d), Rvalue::Use(Operand::Copy(Place::Local(s))))
                if d == s =>
            {
                false
            }
            Statement::Assign(Place::Local(d), rvalue) => read.contains(d) || !is_pure(rvalue),
            _ => true,
        });
        if block.stmts.len() != before {
            changed = true;
        }
    }
    changed
}

/// An rvalue with no observable effect beyond producing its value; safe to drop if the result is
/// unused. Calls and allocations are conservatively impure.
pub(crate) fn is_pure(rvalue: &Rvalue) -> bool {
    matches!(
        rvalue,
        Rvalue::Use(_)
            | Rvalue::Select { .. }
            | Rvalue::Binary(..)
            | Rvalue::Unary(..)
            | Rvalue::ArrayLen(_)
            | Rvalue::StrLen(_)
            | Rvalue::CharAt(..)
            | Rvalue::Concat(..)
            | Rvalue::EnumName { .. }
            | Rvalue::HashCode(_)
            | Rvalue::ToString(_)
            | Rvalue::Cast(..)
            | Rvalue::IsType(..)
            | Rvalue::Discriminant(_)
            | Rvalue::UnionField { .. }
            | Rvalue::FuncRef(_)
    )
}

fn collect_read_locals(func: &MirFunction) -> HashSet<Local> {
    let mut read = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.stmts {
            read_stmt(stmt, &mut read);
        }
        read_terminator(&block.terminator, &mut read);
    }
    read
}

fn read_stmt(stmt: &Statement, read: &mut HashSet<Local>) {
    match stmt {
        Statement::Assign(place, rvalue) => {
            read_place_base(place, read);
            read_rvalue(rvalue, read);
        }
        Statement::Retain(o) | Statement::Release(o) => read_operand(o, read),
        Statement::Call { args, .. } => args.iter().for_each(|a| read_operand(a, read)),
        Statement::InterfaceCall { receiver, args, .. } => {
            read_operand(receiver, read);
            args.iter().for_each(|a| read_operand(a, read));
        }
        Statement::Print { arg, .. } => read_operand(arg, read),
        Statement::Nop => {}
    }
}

/// For a destination place, only the base/index of a projected place is *read* (the local being
/// projected must be live); a bare `Local` destination is a pure write.
fn read_place_base(place: &Place, read: &mut HashSet<Local>) {
    match place {
        Place::Field { base, .. } => {
            read.insert(*base);
        }
        Place::Index { base, index } => {
            read.insert(*base);
            read_operand(index, read);
        }
        Place::Local(_) | Place::Global(_) => {}
    }
}

fn read_rvalue(rvalue: &Rvalue, read: &mut HashSet<Local>) {
    match rvalue {
        Rvalue::Select {
            cond,
            then_val,
            else_val,
        } => {
            read_operand(cond, read);
            read_operand(then_val, read);
            read_operand(else_val, read);
        }
        Rvalue::Use(o)
        | Rvalue::ArrayLen(o)
        | Rvalue::StrLen(o)
        | Rvalue::Cast(o, _, _)
        | Rvalue::IsType(o, _)
        | Rvalue::Discriminant(o)
        | Rvalue::HashCode(o)
        | Rvalue::ToString(o)
        | Rvalue::UnionField { base: o, .. } => read_operand(o, read),
        Rvalue::Binary(_, a, b) | Rvalue::CharAt(a, b) | Rvalue::Concat(a, b) => {
            read_operand(a, read);
            read_operand(b, read);
        }
        Rvalue::EnumName { value, .. } => read_operand(value, read),
        Rvalue::ArrayNew { len, .. } => read_operand(len, read),
        Rvalue::Unary(_, a) => read_operand(a, read),
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::UnionNew { args, .. }
        | Rvalue::ArrayLit { elems: args, .. } => args.iter().for_each(|a| read_operand(a, read)),
        Rvalue::IndirectCall { target, args } => {
            read_operand(target, read);
            args.iter().for_each(|a| read_operand(a, read));
        }
        Rvalue::InterfaceCall { receiver, args, .. } => {
            read_operand(receiver, read);
            args.iter().for_each(|a| read_operand(a, read));
        }
        Rvalue::JsCall {
            target,
            method,
            args,
            ..
        } => {
            read_operand(target, read);
            if let Some(m) = method {
                read_operand(m, read);
            }
            args.iter().for_each(|(a, _)| read_operand(a, read));
        }
        Rvalue::FuncRef(_) => {}
    }
}

fn read_terminator(t: &Terminator, read: &mut HashSet<Local>) {
    match t {
        Terminator::If { cond, .. } => read_operand(cond, read),
        Terminator::Switch { value, .. } => read_operand(value, read),
        Terminator::Return(Some(o)) => read_operand(o, read),
        Terminator::AsyncComplete(Some(o)) => read_operand(o, read),
        Terminator::TailCall { args, .. } => args.iter().for_each(|a| read_operand(a, read)),
        _ => {}
    }
}

fn read_operand(op: &Operand, read: &mut HashSet<Local>) {
    if let Operand::Copy(place) = op {
        match place {
            Place::Local(l) => {
                read.insert(*l);
            }
            Place::Field { base, .. } => {
                read.insert(*base);
            }
            Place::Index { base, index } => {
                read.insert(*base);
                read_operand(index, read);
            }
            Place::Global(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Const, Rvalue, Terminator};

    #[test]
    fn removes_unused_pure_assignment() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let dead = b.new_temp(i.int());
        b.assign(
            Place::Local(dead),
            Rvalue::Use(Operand::Const(Const::Int(99))),
        );
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Int(0)))));
        let mut func = b.finish();
        assert!(Dce.run(&mut func, &i));
        assert!(
            func.blocks[0].stmts.is_empty(),
            "dead assignment should be removed"
        );
    }
}
