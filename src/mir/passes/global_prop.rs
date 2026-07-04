//! Global (cross-block) copy and constant propagation. Where [`super::prop::CopyConstProp`] resets
//! its known-value map at every block boundary, this pass runs a forward dataflow so a constant or
//! copy that provably reaches a block along *every* predecessor path is propagated into it.
//!
//! It is a classic "must" analysis: a block's entry facts are the **intersection** (meet) of its
//! predecessors' exit facts, so a value is only assumed known when all incoming paths agree on it.
//! The per-statement transfer reuses `prop`'s `update_known` (reassigning a local invalidates facts
//! that named it), which keeps the analysis sound across loops: a back edge whose body redefines a
//! local drops that fact from the meet.

use super::prop::{subst_stmt_reads, subst_terminator_reads, update_known};
use super::{cfg, MirPass};
use crate::mir::{Local, MirFunction, Operand, Place};
use crate::types::TypeInterner;
use std::collections::HashMap;

pub struct GlobalProp;

type Facts = HashMap<Local, Operand>;

impl MirPass for GlobalProp {
    fn name(&self) -> &'static str {
        "global-prop"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let n = func.blocks.len();
        if n == 0 {
            return false;
        }
        let preds = cfg::predecessors(func);
        let rpo = cfg::reverse_postorder(func);

        // Forward "available copies/constants" dataflow to a fixpoint.
        let mut entry: Vec<Facts> = vec![Facts::new(); n];
        let mut exit: Vec<Facts> = vec![Facts::new(); n];
        let mut changed = true;
        while changed {
            changed = false;
            for &b in &rpo {
                let bi = b.0 as usize;
                let in_state = if b == func.entry {
                    Facts::new()
                } else {
                    meet(&preds[bi], &exit)
                };
                let mut st = in_state.clone();
                for stmt in &func.block(b).stmts {
                    update_known(stmt, &mut st);
                }
                if !facts_eq(&st, &exit[bi]) {
                    exit[bi] = st;
                    changed = true;
                }
                entry[bi] = in_state;
            }
        }

        // Rewrite reads using each block's entry facts, tracking within-block updates as we go.
        let mut rewrote = false;
        for &b in &rpo {
            let mut st = entry[b.0 as usize].clone();
            let block = func.block_mut(b);
            for stmt in &mut block.stmts {
                rewrote |= subst_stmt_reads(stmt, &st);
                update_known(stmt, &mut st);
            }
            rewrote |= subst_terminator_reads(&mut block.terminator, &st);
        }
        rewrote
    }
}

/// The meet of the predecessors' exit facts: keep a `local -> value` fact only when every
/// predecessor agrees on the identical value.
fn meet(preds: &[crate::mir::BlockId], exit: &[Facts]) -> Facts {
    let mut iter = preds.iter();
    let Some(first) = iter.next() else {
        return Facts::new();
    };
    let mut acc = exit[first.0 as usize].clone();
    for p in iter {
        let other = &exit[p.0 as usize];
        acc.retain(|k, v| other.get(k).is_some_and(|w| operand_eq(v, w)));
        if acc.is_empty() {
            break;
        }
    }
    acc
}

/// Equality over the restricted operand shapes `update_known` ever stores: `Const`s and copies of a
/// local. (`Operand` deliberately has no `PartialEq`, so we compare the cases we rely on.)
fn operand_eq(a: &Operand, b: &Operand) -> bool {
    match (a, b) {
        (Operand::Const(x), Operand::Const(y)) => x == y,
        (Operand::Copy(Place::Local(x)), Operand::Copy(Place::Local(y))) => x == y,
        _ => false,
    }
}

fn facts_eq(a: &Facts, b: &Facts) -> bool {
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| operand_eq(v, w)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Const, Operand, Place, Rvalue, Terminator};

    #[test]
    fn propagates_const_across_blocks() {
        // entry: x = 5; goto next   next: return x   ->   return 5
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let x = b.new_temp(i.int());
        let next = b.new_block();
        b.assign(Place::Local(x), Rvalue::Use(Operand::Const(Const::Int(5))));
        b.terminate(Terminator::Goto(next));
        b.switch_to(next);
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(x)))));
        let mut func = b.finish();
        assert!(GlobalProp.run(&mut func, &i));
        match &func.blocks[1].terminator {
            Terminator::Return(Some(Operand::Const(Const::Int(v)))) => assert_eq!(*v, 5),
            other => panic!("expected propagated const, got {:?}", other),
        }
    }

    #[test]
    fn disagreeing_paths_block_propagation() {
        // entry: if c { x = 1 } else { x = 2 }; join: return x  -> x stays a read (no meet value).
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let x = b.new_temp(i.int());
        let c = b.new_temp(i.bool());
        let then_b = b.new_block();
        let else_b = b.new_block();
        let join = b.new_block();
        b.terminate(Terminator::If {
            cond: Operand::Copy(Place::Local(c)),
            then_blk: then_b,
            else_blk: else_b,
        });
        b.switch_to(then_b);
        b.assign(Place::Local(x), Rvalue::Use(Operand::Const(Const::Int(1))));
        b.terminate(Terminator::Goto(join));
        b.switch_to(else_b);
        b.assign(Place::Local(x), Rvalue::Use(Operand::Const(Const::Int(2))));
        b.terminate(Terminator::Goto(join));
        b.switch_to(join);
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(x)))));
        let mut func = b.finish();
        GlobalProp.run(&mut func, &i);
        assert!(
            matches!(
                &func.blocks[3].terminator,
                Terminator::Return(Some(Operand::Copy(Place::Local(_))))
            ),
            "conflicting values must not be propagated"
        );
    }
}
