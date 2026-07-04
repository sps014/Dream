//! Sparse conditional constant propagation. Unlike plain constant folding, SCCP interleaves
//! constant discovery with block reachability: a branch whose condition is a known constant only
//! makes its taken successor reachable, and definitions on unreachable paths are ignored. This
//! folds correlated conditionals and prunes dead blocks in a single pass, which the
//! fold/simplify/DCE fixpoint would otherwise only reach over several rounds (and sometimes not at
//! all, when an unreachable path would otherwise poison a variable's value).
//!
//! MIR is not SSA, so the constant lattice is intentionally *flow-insensitive per local*: a local is
//! constant only when every one of its definitions in reachable blocks agrees on the same value.
//! This is weaker than SSA-based SCCP but always sound — a local is never assumed constant unless
//! all reaching definitions prove it.

use super::const_fold::fold as fold_rvalue;
use super::prop::{subst_stmt_reads, subst_terminator_reads};
use super::MirPass;
use crate::mir::{
    BlockId, Const, MirFunction, Operand, Place, Rvalue, Statement, Terminator,
};
use crate::types::TypeInterner;
use std::collections::{BTreeMap, HashMap};

pub struct Sccp;

/// The constant lattice for a local: unknown-yet, a proven constant, or over-defined.
#[derive(Clone, PartialEq)]
enum Lat {
    Top,
    Const(Const),
    Bottom,
}

impl MirPass for Sccp {
    fn name(&self) -> &'static str {
        "sccp"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let n = func.blocks.len();
        if n == 0 {
            return false;
        }
        let (reachable, lat) = solve(func);

        // Build the propagatable constant map and rewrite.
        let known: HashMap<crate::mir::Local, Operand> = lat
            .iter()
            .filter_map(|(l, v)| match v {
                Lat::Const(c) => Some((*l, Operand::Const(c.clone()))),
                _ => None,
            })
            .collect();

        let mut changed = false;
        for (i, block) in func.blocks.iter_mut().enumerate() {
            if !reachable[i] {
                if !(block.stmts.is_empty() && matches!(block.terminator, Terminator::Unreachable)) {
                    block.stmts.clear();
                    block.terminator = Terminator::Unreachable;
                    changed = true;
                }
                continue;
            }
            for stmt in &mut block.stmts {
                changed |= subst_stmt_reads(stmt, &known);
            }
            changed |= subst_terminator_reads(&mut block.terminator, &known);
            // Fold a now-constant branch into an unconditional jump.
            if let Some(t) = fold_branch(&block.terminator) {
                block.terminator = t;
                changed = true;
            }
        }
        changed
    }
}

/// Runs the reachability + constant fixpoint, returning which blocks are reachable and each local's
/// lattice value.
fn solve(func: &MirFunction) -> (Vec<bool>, BTreeMap<crate::mir::Local, Lat>) {
    let n = func.blocks.len();
    let mut reachable = vec![false; n];
    reachable[func.entry.0 as usize] = true;

    // Parameters carry caller-provided (unknown) values.
    let mut lat: BTreeMap<crate::mir::Local, Lat> = BTreeMap::new();
    for p in &func.params {
        lat.insert(*p, Lat::Bottom);
    }

    let cap = n * 4 + 16;
    for _ in 0..cap {
        let mut changed = false;

        // Recompute each local's value as the meet of its definitions in reachable blocks.
        let mut next: BTreeMap<crate::mir::Local, Lat> = BTreeMap::new();
        for p in &func.params {
            next.insert(*p, Lat::Bottom);
        }
        for (i, block) in func.blocks.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            for stmt in &block.stmts {
                if let Statement::Assign(Place::Local(l), rv) = stmt {
                    let v = eval_rvalue(rv, &lat);
                    let merged = meet(next.get(l).cloned().unwrap_or(Lat::Top), v);
                    next.insert(*l, merged);
                }
            }
            if let Terminator::Await { dest: Some(d), .. } = &block.terminator {
                next.insert(*d, Lat::Bottom);
            }
        }
        if next != lat {
            lat = next;
            changed = true;
        }

        // Grow reachability along taken edges.
        for i in 0..n {
            if !reachable[i] {
                continue;
            }
            for succ in taken_successors(&func.blocks[i].terminator, &lat) {
                if !reachable[succ.0 as usize] {
                    reachable[succ.0 as usize] = true;
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }
    }
    (reachable, lat)
}

/// The lattice value of an operand under the current assignment.
fn eval_operand(op: &Operand, lat: &BTreeMap<crate::mir::Local, Lat>) -> Lat {
    match op {
        Operand::Const(c) => Lat::Const(c.clone()),
        Operand::Copy(Place::Local(l)) => lat.get(l).cloned().unwrap_or(Lat::Top),
        // Memory reads are never known constants here.
        Operand::Copy(_) => Lat::Bottom,
    }
}

/// The lattice value of an rvalue: constant-foldable arithmetic over constant inputs stays constant;
/// anything touching memory / calls is over-defined.
fn eval_rvalue(rv: &Rvalue, lat: &BTreeMap<crate::mir::Local, Lat>) -> Lat {
    match rv {
        Rvalue::Use(o) => eval_operand(o, lat),
        Rvalue::Binary(op, a, b) => {
            let (la, lb) = (eval_operand(a, lat), eval_operand(b, lat));
            match (la, lb) {
                (Lat::Bottom, _) | (_, Lat::Bottom) => Lat::Bottom,
                (Lat::Const(x), Lat::Const(y)) => {
                    match fold_rvalue(&Rvalue::Binary(*op, Operand::Const(x), Operand::Const(y))) {
                        Some(c) => Lat::Const(c),
                        None => Lat::Bottom,
                    }
                }
                // At least one operand is still Top: delay.
                _ => Lat::Top,
            }
        }
        Rvalue::Unary(op, a) => match eval_operand(a, lat) {
            Lat::Const(x) => match fold_rvalue(&Rvalue::Unary(*op, Operand::Const(x))) {
                Some(c) => Lat::Const(c),
                None => Lat::Bottom,
            },
            Lat::Top => Lat::Top,
            Lat::Bottom => Lat::Bottom,
        },
        _ => Lat::Bottom,
    }
}

fn meet(a: Lat, b: Lat) -> Lat {
    match (a, b) {
        (Lat::Top, x) | (x, Lat::Top) => x,
        (Lat::Const(x), Lat::Const(y)) if x == y => Lat::Const(x),
        _ => Lat::Bottom,
    }
}

/// The successors that can actually be taken given known constant conditions.
fn taken_successors(t: &Terminator, lat: &BTreeMap<crate::mir::Local, Lat>) -> Vec<BlockId> {
    match t {
        Terminator::If {
            cond,
            then_blk,
            else_blk,
        } => match eval_operand(cond, lat) {
            Lat::Const(Const::Bool(true)) => vec![*then_blk],
            Lat::Const(Const::Bool(false)) => vec![*else_blk],
            _ => vec![*then_blk, *else_blk],
        },
        Terminator::Switch {
            value,
            targets,
            default,
        } => match eval_operand(value, lat) {
            Lat::Const(Const::Int(v)) => {
                let tgt = targets
                    .iter()
                    .find(|(k, _)| *k == v)
                    .map(|(_, b)| *b)
                    .unwrap_or(*default);
                vec![tgt]
            }
            _ => t.successors(),
        },
        _ => t.successors(),
    }
}

/// If a branch's condition is a constant (after substitution), the equivalent unconditional jump.
fn fold_branch(t: &Terminator) -> Option<Terminator> {
    match t {
        Terminator::If {
            cond: Operand::Const(Const::Bool(b)),
            then_blk,
            else_blk,
        } => Some(Terminator::Goto(if *b { *then_blk } else { *else_blk })),
        Terminator::Switch {
            value: Operand::Const(Const::Int(v)),
            targets,
            default,
        } => {
            let tgt = targets
                .iter()
                .find(|(k, _)| k == v)
                .map(|(_, b)| *b)
                .unwrap_or(*default);
            Some(Terminator::Goto(tgt))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Operand, Place, Rvalue};

    /// `c = true; if c { return 1 } else { return 2 }` — SCCP proves the else block unreachable.
    #[test]
    fn prunes_dead_branch() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let c = b.new_temp(i.bool());
        let then_b = b.new_block();
        let else_b = b.new_block();
        b.assign(Place::Local(c), Rvalue::Use(Operand::Const(Const::Bool(true))));
        b.terminate(Terminator::If {
            cond: Operand::Copy(Place::Local(c)),
            then_blk: then_b,
            else_blk: else_b,
        });
        b.switch_to(then_b);
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Int(1)))));
        b.switch_to(else_b);
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Int(2)))));
        let mut func = b.finish();
        assert!(Sccp.run(&mut func, &i));
        assert!(
            matches!(func.blocks[0].terminator, Terminator::Goto(t) if t == then_b),
            "branch should fold to the then-block"
        );
        assert!(
            matches!(func.blocks[2].terminator, Terminator::Unreachable),
            "else block should be pruned"
        );
    }
}
