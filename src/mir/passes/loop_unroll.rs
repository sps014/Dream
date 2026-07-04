//! Full unrolling of small, statically-counted loops. When a natural loop is a simple counted loop
//! — one induction variable initialized to a constant before the loop, tested against a constant
//! bound in the header, and incremented by a positive constant on the single latch — and the total
//! trip count is a small constant, the loop is replaced by that many straight-line copies of its
//! body. This removes the per-iteration branch and back edge and exposes the copies to constant
//! folding / CSE.
//!
//! The matcher is deliberately narrow (so it is easy to keep sound): the body must be a single
//! straight-line chain of blocks (no internal branching), the header must contain only the loop
//! comparison, and the induction variable must have exactly two definitions (the pre-loop init and
//! the latch increment). Anything else is left untouched. Body statements — including calls and RC
//! `Retain`/`Release` — are cloned verbatim per iteration, so effects and refcount balance match the
//! original loop exactly.

use super::cfg::{self, DomTree};
use super::licm::{stmt_reads, terminator_reads};
use super::MirPass;
use crate::mir::{
    BasicBlock, BinOp, BlockId, Const, Local, MirFunction, Operand, Place, Rvalue, Statement,
    Terminator,
};
use crate::types::TypeInterner;
use std::collections::BTreeSet;

/// Never unroll beyond this many iterations, or this many total cloned statements, to keep code size
/// bounded.
const MAX_TRIPS: i64 = 8;
const MAX_TOTAL_STMTS: usize = 96;

pub struct LoopUnroll;

impl MirPass for LoopUnroll {
    fn name(&self) -> &'static str {
        "loop-unroll"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        // Unroll one loop per call; the pass-manager fixpoint reruns for the rest with fresh CFG
        // analysis. Bounded so it can never spin.
        let mut changed = false;
        for _ in 0..func.blocks.len() + 1 {
            if unroll_one(func) {
                changed = true;
            } else {
                break;
            }
        }
        changed
    }
}

/// The parts of a matched counted loop needed to unroll it.
struct Counted {
    header: BlockId,
    exit: BlockId,
    chain: Vec<BlockId>,
    body: BTreeSet<BlockId>,
    trips: i64,
}

fn unroll_one(func: &mut MirFunction) -> bool {
    let Some(c) = find_counted_loop(func) else {
        return false;
    };

    // One iteration's statements = the chain blocks' statements in order (the header's comparison is
    // dropped, since the trip count is now known).
    let mut iter_stmts: Vec<Statement> = Vec::new();
    for &b in &c.chain {
        iter_stmts.extend(func.block(b).stmts.iter().cloned());
    }

    // The unrolled straight-line block: `trips` copies, then fall through to the loop exit.
    let mut stmts = Vec::with_capacity(iter_stmts.len() * c.trips as usize);
    for _ in 0..c.trips {
        stmts.extend(iter_stmts.iter().cloned());
    }
    let unrolled = BlockId(func.blocks.len() as u32);
    func.blocks.push(BasicBlock {
        stmts,
        terminator: Terminator::Goto(c.exit),
    });

    // Route every edge that entered the loop header (from outside the loop) into the unrolled block.
    let n = func.blocks.len();
    for i in 0..n {
        let bid = BlockId(i as u32);
        if bid == unrolled || c.body.contains(&bid) {
            continue;
        }
        redirect(&mut func.blocks[i].terminator, c.header, unrolled);
    }
    if func.entry == c.header {
        func.entry = unrolled;
    }
    // The old loop blocks are now unreachable; DCE will drop them.
    true
}

/// Matches the narrow counted-loop shape and computes the trip count, or `None`.
fn find_counted_loop(func: &MirFunction) -> Option<Counted> {
    let loops = cfg::natural_loops(func);
    let dom = DomTree::new(func);

    for l in &loops {
        if l.latches.len() != 1 {
            continue;
        }
        let header = l.header;
        let latch = l.latches[0];
        if latch == header {
            continue; // single-block self-loop: not handled
        }

        // Header must be exactly `cmp = iv <|<= bound; if cmp goto entry else exit`.
        let hblock = func.block(header);
        if hblock.stmts.len() != 1 {
            continue;
        }
        let Statement::Assign(Place::Local(cmp), Rvalue::Binary(cmpop, a, b)) = &hblock.stmts[0]
        else {
            continue;
        };
        if !matches!(cmpop, BinOp::Lt | BinOp::Le) {
            continue;
        }
        let (Operand::Copy(Place::Local(iv)), Operand::Const(Const::Int(bound))) = (a, b) else {
            continue;
        };
        let (iv, bound, cmpop) = (*iv, *bound, *cmpop);
        let Terminator::If {
            cond: Operand::Copy(Place::Local(cmp_read)),
            then_blk,
            else_blk,
        } = &hblock.terminator
        else {
            continue;
        };
        if cmp_read != cmp {
            continue;
        }
        let (entry, exit) = (*then_blk, *else_blk);
        if !l.body.contains(&entry) || l.body.contains(&exit) || entry == header {
            continue;
        }
        // `cmp` must be used nowhere but this branch, so dropping its computation is safe.
        if local_used_outside(func, *cmp, header) {
            continue;
        }

        // Body (minus header) must be a single straight-line chain ending at the latch->header edge.
        let Some(chain) = straight_chain(func, &l.body, entry, header, latch) else {
            continue;
        };

        // Induction variable: exactly two defs overall — the latch increment (+const) and a pre-loop
        // constant init that dominates the header.
        let Some((step, start)) = induction(func, &dom, &l.body, iv, header) else {
            continue;
        };
        if step <= 0 {
            continue;
        }

        let trips = trip_count(start, bound, step, cmpop);
        if !(0..=MAX_TRIPS).contains(&trips) {
            continue;
        }
        let body_stmts: usize = chain.iter().map(|b| func.block(*b).stmts.len()).sum();
        if body_stmts.saturating_mul(trips as usize) > MAX_TOTAL_STMTS {
            continue;
        }

        return Some(Counted {
            header,
            exit,
            chain,
            body: l.body.clone(),
            trips,
        });
    }
    None
}

/// The number of times `iv in [start, ..) stepping by step>0` satisfies `iv < bound` (Lt) or
/// `iv <= bound` (Le).
fn trip_count(start: i64, bound: i64, step: i64, cmpop: BinOp) -> i64 {
    let last_inclusive = match cmpop {
        BinOp::Lt => bound - 1,
        BinOp::Le => bound,
        _ => return -1,
    };
    if start > last_inclusive {
        return 0;
    }
    (last_inclusive - start) / step + 1
}

/// Follows single-successor `Goto` edges from `entry` through the loop body until the latch's edge
/// back to the header, returning the chain of blocks (which must cover the whole body except the
/// header).
fn straight_chain(
    func: &MirFunction,
    body: &BTreeSet<BlockId>,
    entry: BlockId,
    header: BlockId,
    latch: BlockId,
) -> Option<Vec<BlockId>> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cur = entry;
    loop {
        if cur == header || !body.contains(&cur) || !seen.insert(cur) {
            return None;
        }
        chain.push(cur);
        match &func.block(cur).terminator {
            Terminator::Goto(t) if *t == header => {
                if cur != latch {
                    return None;
                }
                break;
            }
            Terminator::Goto(t) if body.contains(t) && *t != header => cur = *t,
            _ => return None,
        }
    }
    if chain.len() != body.len() - 1 {
        return None;
    }
    Some(chain)
}

/// Verifies `iv` has exactly two definitions — a `+ step` increment inside the loop and a constant
/// init outside it that dominates the header — and returns `(step, start)`.
fn induction(
    func: &MirFunction,
    dom: &DomTree,
    body: &BTreeSet<BlockId>,
    iv: Local,
    header: BlockId,
) -> Option<(i64, i64)> {
    let mut step: Option<i64> = None;
    let mut start: Option<i64> = None;
    let mut def_count = 0u32;

    for (i, block) in func.blocks.iter().enumerate() {
        let bid = BlockId(i as u32);
        for stmt in &block.stmts {
            if let Statement::Assign(Place::Local(d), rv) = stmt {
                if *d != iv {
                    continue;
                }
                def_count += 1;
                if body.contains(&bid) {
                    // In-loop def must be the `iv = iv + const` increment.
                    match rv {
                        Rvalue::Binary(BinOp::Add, Operand::Copy(Place::Local(s)), Operand::Const(Const::Int(k)))
                            if *s == iv =>
                        {
                            step = Some(*k);
                        }
                        _ => return None,
                    }
                } else {
                    // Out-of-loop def must be a constant init dominating the header.
                    match rv {
                        Rvalue::Use(Operand::Const(Const::Int(v))) if dom.dominates(bid, header) => {
                            start = Some(*v);
                        }
                        _ => return None,
                    }
                }
            }
        }
        if let Terminator::Await { dest: Some(d), .. } = &block.terminator {
            if *d == iv {
                return None; // async-bound induction var: bail
            }
        }
    }
    if def_count != 2 {
        return None;
    }
    Some((step?, start?))
}

/// True if `local` is read anywhere except the terminator of `only_block`.
fn local_used_outside(func: &MirFunction, local: Local, only_block: BlockId) -> bool {
    for (i, block) in func.blocks.iter().enumerate() {
        let bid = BlockId(i as u32);
        let mut hit = false;
        for stmt in &block.stmts {
            stmt_reads(stmt, &mut |l| {
                if l == local {
                    hit = true;
                }
            });
        }
        if bid != only_block {
            terminator_reads(&block.terminator, &mut |l| {
                if l == local {
                    hit = true;
                }
            });
        }
        if hit {
            return true;
        }
    }
    false
}

/// Replaces every successor edge equal to `from` with `to` in a terminator.
fn redirect(t: &mut Terminator, from: BlockId, to: BlockId) {
    let fix = |b: &mut BlockId| {
        if *b == from {
            *b = to;
        }
    };
    match t {
        Terminator::Goto(b) => fix(b),
        Terminator::If {
            then_blk, else_blk, ..
        } => {
            fix(then_blk);
            fix(else_blk);
        }
        Terminator::Switch {
            targets, default, ..
        } => {
            for (_, b) in targets {
                fix(b);
            }
            fix(default);
        }
        Terminator::Await { resume, .. } => fix(resume),
        Terminator::Return(_)
        | Terminator::AsyncComplete(_)
        | Terminator::TailCall { .. }
        | Terminator::Unreachable => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;

    /// Builds the canonical counted loop
    ///   init:  iv = 0; goto header
    ///   header: cmp = iv < 3; if cmp goto body else exit
    ///   body:  acc = acc + iv; goto step
    ///   step:  iv = iv + 1; goto header
    ///   exit:  return acc
    /// which should fully unroll to 3 copies of {body; step}.
    #[test]
    fn unrolls_small_counted_loop() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let iv = b.new_temp(i.int());
        let acc = b.new_temp(i.int());
        let cmp = b.new_temp(i.bool());
        let header = b.new_block();
        let body = b.new_block();
        let step = b.new_block();
        let exit = b.new_block();

        b.assign(Place::Local(iv), Rvalue::Use(Operand::Const(Const::Int(0))));
        b.terminate(Terminator::Goto(header));
        b.switch_to(header);
        b.assign(
            Place::Local(cmp),
            Rvalue::Binary(BinOp::Lt, Operand::Copy(Place::Local(iv)), Operand::Const(Const::Int(3))),
        );
        b.terminate(Terminator::If {
            cond: Operand::Copy(Place::Local(cmp)),
            then_blk: body,
            else_blk: exit,
        });
        b.switch_to(body);
        b.assign(
            Place::Local(acc),
            Rvalue::Binary(BinOp::Add, Operand::Copy(Place::Local(acc)), Operand::Copy(Place::Local(iv))),
        );
        b.terminate(Terminator::Goto(step));
        b.switch_to(step);
        b.assign(
            Place::Local(iv),
            Rvalue::Binary(BinOp::Add, Operand::Copy(Place::Local(iv)), Operand::Const(Const::Int(1))),
        );
        b.terminate(Terminator::Goto(header));
        b.switch_to(exit);
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(acc)))));
        let mut func = b.finish();

        assert!(LoopUnroll.run(&mut func, &i), "canonical loop should unroll");
        // The unrolled block holds 3 copies of {acc+=iv ; iv+=1} = 6 statements, ending at exit.
        let unrolled = func
            .blocks
            .iter()
            .find(|bb| matches!(bb.terminator, Terminator::Goto(t) if t == exit) && bb.stmts.len() == 6);
        assert!(unrolled.is_some(), "expected a 6-statement unrolled block");
    }

    #[test]
    fn does_not_unroll_unknown_bound() {
        // Same shape but bound is a parameter (non-constant): must not unroll.
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let n = b.new_param(i.int(), Some("n".into()));
        let iv = b.new_temp(i.int());
        let cmp = b.new_temp(i.bool());
        let header = b.new_block();
        let body = b.new_block();
        let exit = b.new_block();
        b.assign(Place::Local(iv), Rvalue::Use(Operand::Const(Const::Int(0))));
        b.terminate(Terminator::Goto(header));
        b.switch_to(header);
        b.assign(
            Place::Local(cmp),
            Rvalue::Binary(BinOp::Lt, Operand::Copy(Place::Local(iv)), Operand::Copy(Place::Local(n))),
        );
        b.terminate(Terminator::If {
            cond: Operand::Copy(Place::Local(cmp)),
            then_blk: body,
            else_blk: exit,
        });
        b.switch_to(body);
        b.assign(
            Place::Local(iv),
            Rvalue::Binary(BinOp::Add, Operand::Copy(Place::Local(iv)), Operand::Const(Const::Int(1))),
        );
        b.terminate(Terminator::Goto(header));
        b.switch_to(exit);
        b.terminate(Terminator::Return(None));
        let mut func = b.finish();
        assert!(!LoopUnroll.run(&mut func, &i), "non-constant bound must not unroll");
    }
}
