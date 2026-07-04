//! CFG simplification: fold branches with constant conditions into unconditional jumps, resolve
//! constant `switch`es, collapse branches whose arms all target the same block, thread jumps through
//! empty forwarding blocks, and merge a block into its sole predecessor. Subsumes the codegen-time
//! `is`-folding the old backend did inline.

use super::MirPass;
use crate::mir::{BlockId, Const, MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use crate::types::TypeInterner;
use std::collections::{HashMap, HashSet};

pub struct SimplifyCfg;

impl MirPass for SimplifyCfg {
    fn name(&self) -> &'static str {
        "simplify-cfg"
    }

    fn run(&self, func: &mut MirFunction, interner: &TypeInterner) -> bool {
        let mut changed = fold_constant_branches(func);
        changed |= collapse_same_target_branches(func);
        changed |= thread_empty_jumps(func);
        changed |= thread_branch_targets(func);
        changed |= if_convert(func, interner);
        changed |= merge_single_pred_blocks(func);
        changed
    }
}

/// Jump threading: redirects every edge that targets an empty forwarding block (`{ } goto Z`)
/// straight to `Z`, following chains. Unlike `thread_empty_jumps` (which only rewrites `Goto`
/// terminators), this threads the targets of `If`/`Switch`/`Await` too.
fn thread_branch_targets(func: &mut MirFunction) -> bool {
    // One-hop forwarding target of each block (empty block ending in a non-self `Goto`).
    let forward: Vec<Option<BlockId>> = (0..func.blocks.len())
        .map(|i| {
            let blk = &func.blocks[i];
            match blk.terminator {
                Terminator::Goto(t) if blk.stmts.is_empty() && t.0 as usize != i => Some(t),
                _ => None,
            }
        })
        .collect();

    let chase = |start: BlockId| -> BlockId {
        let mut cur = start;
        let mut seen = HashSet::new();
        while seen.insert(cur) {
            match forward[cur.0 as usize] {
                Some(t) => cur = t,
                None => break,
            }
        }
        cur
    };

    let mut changed = false;
    for i in 0..func.blocks.len() {
        let mut edit = |b: &mut BlockId| {
            let t = chase(*b);
            if t != *b {
                *b = t;
                changed = true;
            }
        };
        match &mut func.blocks[i].terminator {
            Terminator::Goto(b) => edit(b),
            Terminator::If {
                then_blk, else_blk, ..
            } => {
                edit(then_blk);
                edit(else_blk);
            }
            Terminator::Switch {
                targets, default, ..
            } => {
                for (_, b) in targets {
                    edit(b);
                }
                edit(default);
            }
            Terminator::Await { resume, .. } => edit(resume),
            Terminator::Return(_)
            | Terminator::AsyncComplete(_)
            | Terminator::TailCall { .. }
            | Terminator::Unreachable => {}
        }
    }
    changed
}

/// If-conversion: collapses a diamond that only chooses between two side-effect-free scalar values
/// into a branchless `select`.
///
/// `H: if cond goto T else E`, where `T = { x = a; goto J }` and `E = { x = b; goto J }` (same `x`
/// and `J`, each with `H` as its only predecessor, `a`/`b` constants or plain local reads, `x` a
/// scalar) becomes `H: x = select(cond, a, b); goto J`, and `T`/`E` are dropped.
fn if_convert(func: &mut MirFunction, interner: &TypeInterner) -> bool {
    let preds = predecessor_counts(func);
    let mut changed = false;
    for i in 0..func.blocks.len() {
        let Terminator::If {
            cond,
            then_blk,
            else_blk,
        } = &func.blocks[i].terminator
        else {
            continue;
        };
        let (cond, t, e) = (cond.clone(), *then_blk, *else_blk);
        if t == e || t.0 as usize == i || e.0 as usize == i {
            continue;
        }
        // Each arm must be exclusively reached from this branch.
        if preds.get(&t).copied().unwrap_or(0) != 1 || preds.get(&e).copied().unwrap_or(0) != 1 {
            continue;
        }
        let Some((xt, at, jt)) = single_select_arm(func, t) else {
            continue;
        };
        let Some((xe, ae, je)) = single_select_arm(func, e) else {
            continue;
        };
        if xt != xe || jt != je {
            continue;
        }
        if interner.is_value_type(func.local_ty(xt)) {
            continue; // `select` is scalar-only
        }

        func.blocks[i].stmts.push(Statement::Assign(
            Place::Local(xt),
            Rvalue::Select {
                cond,
                then_val: at,
                else_val: ae,
            },
        ));
        func.blocks[i].terminator = Terminator::Goto(jt);
        // Detach the now-dead arms (DCE reclaims them).
        for arm in [t, e] {
            let blk = &mut func.blocks[arm.0 as usize];
            blk.stmts.clear();
            blk.terminator = Terminator::Unreachable;
        }
        changed = true;
    }
    changed
}

/// Matches an arm block of the form `{ x = <const|local>; goto J }`, returning `(x, value, J)`.
fn single_select_arm(func: &MirFunction, b: BlockId) -> Option<(crate::mir::Local, Operand, BlockId)> {
    let block = func.block(b);
    if block.stmts.len() != 1 {
        return None;
    }
    let Statement::Assign(Place::Local(x), Rvalue::Use(val)) = &block.stmts[0] else {
        return None;
    };
    // Only pure, always-safe-to-evaluate operands (no memory loads that could fault when the branch
    // would not have taken them).
    if !matches!(val, Operand::Const(_) | Operand::Copy(Place::Local(_))) {
        return None;
    }
    let Terminator::Goto(j) = block.terminator else {
        return None;
    };
    Some((*x, val.clone(), j))
}

/// Collapses a branch whose outcomes all jump to the same block into an unconditional `Goto` (the
/// condition/scrutinee then becomes dead and is removed by DCE).
fn collapse_same_target_branches(func: &mut MirFunction) -> bool {
    let mut changed = false;
    for block in &mut func.blocks {
        let new_term = match &block.terminator {
            Terminator::If {
                then_blk, else_blk, ..
            } if then_blk == else_blk => Some(Terminator::Goto(*then_blk)),
            Terminator::Switch {
                targets, default, ..
            } if targets.iter().all(|(_, b)| b == default) => Some(Terminator::Goto(*default)),
            _ => None,
        };
        if let Some(t) = new_term {
            block.terminator = t;
            changed = true;
        }
    }
    changed
}

/// Merges a block `t` into its unique predecessor `i` when `i` ends in `goto t` and `t` has exactly
/// one predecessor: `t`'s statements and terminator are appended to `i`, and `t` is left empty for
/// DCE to drop. Recomputes predecessors after each merge (identities shift) until a fixpoint.
fn merge_single_pred_blocks(func: &mut MirFunction) -> bool {
    let mut any = false;
    loop {
        let preds = predecessor_counts(func);
        let mut merged = false;
        for i in 0..func.blocks.len() {
            let Terminator::Goto(t) = func.blocks[i].terminator else {
                continue;
            };
            let ti = t.0 as usize;
            if ti == i || t == func.entry || preds.get(&t).copied().unwrap_or(0) != 1 {
                continue;
            }
            let t_stmts = std::mem::take(&mut func.blocks[ti].stmts);
            let t_term =
                std::mem::replace(&mut func.blocks[ti].terminator, Terminator::Unreachable);
            func.blocks[i].stmts.extend(t_stmts);
            func.blocks[i].terminator = t_term;
            merged = true;
            any = true;
            break;
        }
        if !merged {
            break;
        }
    }
    any
}

/// Counts how many terminators name each block as a successor (its in-degree).
fn predecessor_counts(func: &MirFunction) -> HashMap<BlockId, usize> {
    let mut counts: HashMap<BlockId, usize> = HashMap::new();
    for block in &func.blocks {
        for succ in block.terminator.successors() {
            *counts.entry(succ).or_default() += 1;
        }
    }
    counts
}

fn fold_constant_branches(func: &mut MirFunction) -> bool {
    let mut changed = false;
    for block in &mut func.blocks {
        let new_term = match &block.terminator {
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
                let target = targets
                    .iter()
                    .find(|(k, _)| k == v)
                    .map(|(_, b)| *b)
                    .unwrap_or(*default);
                Some(Terminator::Goto(target))
            }
            _ => None,
        };
        if let Some(t) = new_term {
            block.terminator = t;
            changed = true;
        }
    }
    changed
}

/// Replaces `goto t` with `t`'s terminator when `t` is an empty forwarding block, collapsing chains
/// of trivial jumps. Self-targets are left alone to avoid spinning on empty self-loops.
fn thread_empty_jumps(func: &mut MirFunction) -> bool {
    let mut changed = false;
    for i in 0..func.blocks.len() {
        let here = BlockId(i as u32);
        if let Terminator::Goto(t) = func.blocks[i].terminator {
            if t != here && func.block(t).stmts.is_empty() {
                let forwarded = func.block(t).terminator.clone();
                // Only thread when it actually changes the target (avoid no-op churn / cycles).
                if !matches!(&forwarded, Terminator::Goto(u) if *u == t) {
                    func.blocks[i].terminator = forwarded;
                    changed = true;
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;

    #[test]
    fn folds_constant_if() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let then_blk = b.new_block();
        let else_blk = b.new_block();
        b.terminate(Terminator::If {
            cond: Operand::Const(Const::Bool(true)),
            then_blk,
            else_blk,
        });
        b.switch_to(then_blk);
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Int(1)))));
        b.switch_to(else_blk);
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Int(2)))));
        let mut func = b.finish();
        assert!(SimplifyCfg.run(&mut func, &i));
        // The constant `if` folds to a jump into the then-block, which (being empty) is threaded
        // through to its `return 1` terminator.
        match &func.blocks[0].terminator {
            Terminator::Goto(t) => assert_eq!(*t, then_blk),
            Terminator::Return(Some(Operand::Const(Const::Int(1)))) => {}
            other => panic!("expected then-branch path, got {:?}", other),
        }
    }
}
