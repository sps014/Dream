//! Loop-invariant code motion. For each natural loop, an assignment whose right-hand side is
//! loop-invariant (all inputs defined outside the loop) and pure & non-trapping is hoisted into a
//! synthesized preheader that runs once before the loop.
//!
//! Safety conditions (all required, so the move never changes observable behavior):
//! - The rvalue is `Use`/`Unary`/`Binary` over constants and locals defined outside the loop. Only
//!   non-trapping ops are eligible (`Div`/`Rem` are excluded, since speculating a trap before the
//!   loop would be observable); calls/allocations/casts/memory reads are never invariant here.
//! - The destination local is assigned exactly once in the loop, is used nowhere outside it, and
//!   every in-loop use is dominated by the def. Together these rule out loop-carried reads and
//!   partially-executed defs, so hoisting the single invariant value is semantics-preserving.
//!
//! The preheader is created by redirecting every out-of-loop edge into the header through a fresh
//! block; latch (back) edges keep targeting the header. Empty leftover statements become `Nop`s for
//! DCE to clear.

use super::cfg::{self, DomTree};
use super::MirPass;
use crate::mir::{
    BasicBlock, BinOp, BlockId, Local, MirFunction, Operand, Place, Rvalue, Statement, Terminator,
};
use crate::types::TypeInterner;
use std::collections::{BTreeSet, HashMap, HashSet};

pub struct Licm;

impl MirPass for Licm {
    fn name(&self) -> &'static str {
        "licm"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = false;
        // Re-derive loops/dominators after each hoist (the CFG changes), processing one loop at a
        // time. Capped so a bug can never spin forever.
        for _ in 0..func.blocks.len() + 1 {
            if hoist_one_loop(func) {
                changed = true;
            } else {
                break;
            }
        }
        changed
    }
}

/// Hoists the invariant statements of the first loop that has any, returning whether it did.
fn hoist_one_loop(func: &mut MirFunction) -> bool {
    let loops = cfg::natural_loops(func);
    if loops.is_empty() {
        return false;
    }
    let dom = DomTree::new(func);
    let use_blocks = use_block_map(func);

    for l in &loops {
        let body = &l.body;
        let def_counts = def_counts_in(func, body);

        // Collect hoistable (block, stmt-index) candidates in deterministic order.
        let mut candidates: Vec<(BlockId, usize, Local)> = Vec::new();
        for &b in body {
            let block = func.block(b);
            for (idx, stmt) in block.stmts.iter().enumerate() {
                let Statement::Assign(Place::Local(d), rv) = stmt else {
                    continue;
                };
                let Some(operands) = hoistable_operands(rv) else {
                    continue;
                };
                if def_counts.get(d).copied().unwrap_or(0) != 1 {
                    continue;
                }
                if !operands.iter().all(|o| operand_invariant(o, &def_counts)) {
                    continue;
                }
                if !dest_safe_to_hoist(func, &dom, body, &use_blocks, *d, b, idx) {
                    continue;
                }
                candidates.push((b, idx, *d));
            }
        }
        if candidates.is_empty() {
            continue;
        }

        build_preheader_and_hoist(func, l.header, body, &candidates);
        return true;
    }
    false
}

/// A local is safe to hoist when it is used only inside the loop and every use is dominated by its
/// single def (so no loop-carried read observes a different value once the def moves to the
/// preheader).
fn dest_safe_to_hoist(
    func: &MirFunction,
    dom: &DomTree,
    body: &BTreeSet<BlockId>,
    use_blocks: &HashMap<Local, BTreeSet<BlockId>>,
    d: Local,
    def_block: BlockId,
    def_idx: usize,
) -> bool {
    if let Some(uses) = use_blocks.get(&d) {
        for &u in uses {
            if !body.contains(&u) {
                return false; // used outside the loop (live-out): moving would change it
            }
            if u == def_block {
                // Same block: any read *before* the def would see a loop-carried value.
                if reads_before(func.block(def_block), d, def_idx) {
                    return false;
                }
            } else if !dom.dominates(def_block, u) {
                return false; // in-loop use not dominated by the def
            }
        }
    }
    true
}

/// True if `d` is read among the first `before` statements of `block`.
fn reads_before(block: &BasicBlock, d: Local, before: usize) -> bool {
    let mut hit = false;
    for stmt in block.stmts.iter().take(before) {
        stmt_reads(stmt, &mut |l| {
            if l == d {
                hit = true;
            }
        });
    }
    hit
}

/// Rvalue operands eligible for hoisting, or `None` if the rvalue kind can trap / touch memory /
/// have side effects and so must never be speculated before the loop.
fn hoistable_operands(rv: &Rvalue) -> Option<Vec<&Operand>> {
    match rv {
        Rvalue::Use(o) | Rvalue::Unary(_, o) => Some(vec![o]),
        Rvalue::Binary(op, a, b) if !matches!(op, BinOp::Div | BinOp::Rem) => Some(vec![a, b]),
        _ => None,
    }
}

/// An operand is loop-invariant if it is a constant or reads a local not defined in the loop.
/// Memory reads (field/index/global) are conservatively treated as variant.
fn operand_invariant(o: &Operand, defs_in_loop: &HashMap<Local, u32>) -> bool {
    match o {
        Operand::Const(_) => true,
        Operand::Copy(Place::Local(l)) => !defs_in_loop.contains_key(l),
        _ => false,
    }
}

/// Counts how many times each local is assigned inside the loop body (definitions via `Assign` to a
/// local, or an `Await` result binding).
fn def_counts_in(func: &MirFunction, body: &BTreeSet<BlockId>) -> HashMap<Local, u32> {
    let mut counts: HashMap<Local, u32> = HashMap::new();
    for &b in body {
        let block = func.block(b);
        for stmt in &block.stmts {
            if let Statement::Assign(Place::Local(d), _) = stmt {
                *counts.entry(*d).or_default() += 1;
            }
        }
        if let Terminator::Await { dest: Some(d), .. } = &block.terminator {
            *counts.entry(*d).or_default() += 1;
        }
    }
    counts
}

/// Creates a preheader for `header`, moves the candidate statements into it (in order), and clears
/// them from their source blocks.
fn build_preheader_and_hoist(
    func: &mut MirFunction,
    header: BlockId,
    body: &BTreeSet<BlockId>,
    candidates: &[(BlockId, usize, Local)],
) {
    // Clone the statements to hoist, then blank the originals.
    let mut hoisted: Vec<Statement> = Vec::with_capacity(candidates.len());
    let mut to_remove: HashMap<BlockId, HashSet<usize>> = HashMap::new();
    for &(b, idx, _) in candidates {
        hoisted.push(func.block(b).stmts[idx].clone());
        to_remove.entry(b).or_default().insert(idx);
    }
    for (b, idxs) in &to_remove {
        let block = func.block_mut(*b);
        let mut i = 0;
        block.stmts.retain(|_| {
            let keep = !idxs.contains(&i);
            i += 1;
            keep
        });
    }

    // New preheader jumps to the header; hoisted code runs there.
    let ph = BlockId(func.blocks.len() as u32);
    func.blocks.push(BasicBlock {
        stmts: hoisted,
        terminator: Terminator::Goto(header),
    });

    // Redirect every out-of-loop edge that targeted the header to go through the preheader.
    let n = func.blocks.len();
    for i in 0..n {
        let bid = BlockId(i as u32);
        if bid == ph || body.contains(&bid) {
            continue;
        }
        redirect(&mut func.blocks[i].terminator, header, ph);
    }
    if func.entry == header {
        func.entry = ph;
    }
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

/// Maps each local to the set of blocks that *read* it anywhere in the function.
fn use_block_map(func: &MirFunction) -> HashMap<Local, BTreeSet<BlockId>> {
    let mut map: HashMap<Local, BTreeSet<BlockId>> = HashMap::new();
    for (i, block) in func.blocks.iter().enumerate() {
        let bid = BlockId(i as u32);
        let mut record = |l: Local| {
            map.entry(l).or_default().insert(bid);
        };
        for stmt in &block.stmts {
            stmt_reads(stmt, &mut record);
        }
        terminator_reads(&block.terminator, &mut record);
    }
    map
}

pub(super) fn stmt_reads(stmt: &Statement, f: &mut impl FnMut(Local)) {
    match stmt {
        Statement::Assign(place, rv) => {
            place_base_reads(place, f);
            rvalue_reads(rv, f);
        }
        Statement::Retain(o) | Statement::Release(o) => operand_reads(o, f),
        Statement::Call { args, .. } => args.iter().for_each(|a| operand_reads(a, f)),
        Statement::InterfaceCall { receiver, args, .. } => {
            operand_reads(receiver, f);
            args.iter().for_each(|a| operand_reads(a, f));
        }
        Statement::Print { arg, .. } => operand_reads(arg, f),
        Statement::Nop | Statement::DebugLine(_) => {}
    }
}

fn place_base_reads(place: &Place, f: &mut impl FnMut(Local)) {
    match place {
        Place::Field { base, .. } => f(*base),
        Place::Index { base, index } => {
            f(*base);
            operand_reads(index, f);
        }
        Place::Local(_) | Place::Global(_) => {}
    }
}

fn rvalue_reads(rv: &Rvalue, f: &mut impl FnMut(Local)) {
    match rv {
        Rvalue::Select {
            cond,
            then_val,
            else_val,
        } => {
            operand_reads(cond, f);
            operand_reads(then_val, f);
            operand_reads(else_val, f);
        }
        Rvalue::Use(o)
        | Rvalue::ArrayLen(o)
        | Rvalue::StrLen(o)
        | Rvalue::Cast(o, _, _)
        | Rvalue::IsType(o, _)
        | Rvalue::Discriminant(o)
        | Rvalue::HashCode(o)
        | Rvalue::ToString(o)
        | Rvalue::UnionField { base: o, .. } => operand_reads(o, f),
        Rvalue::Binary(_, a, b) | Rvalue::CharAt(a, b) | Rvalue::Concat(a, b) => {
            operand_reads(a, f);
            operand_reads(b, f);
        }
        Rvalue::EnumName { value, .. } => operand_reads(value, f),
        Rvalue::ArrayNew { len, .. } => operand_reads(len, f),
        Rvalue::Unary(_, a) => operand_reads(a, f),
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::UnionNew { args, .. }
        | Rvalue::ArrayLit { elems: args, .. } => args.iter().for_each(|a| operand_reads(a, f)),
        Rvalue::IndirectCall { target, args } => {
            operand_reads(target, f);
            args.iter().for_each(|a| operand_reads(a, f));
        }
        Rvalue::InterfaceCall { receiver, args, .. } => {
            operand_reads(receiver, f);
            args.iter().for_each(|a| operand_reads(a, f));
        }
        Rvalue::JsCall {
            target,
            method,
            args,
            ..
        } => {
            operand_reads(target, f);
            if let Some(m) = method {
                operand_reads(m, f);
            }
            args.iter().for_each(|(a, _)| operand_reads(a, f));
        }
        Rvalue::FuncRef(_) => {}
    }
}

pub(super) fn terminator_reads(t: &Terminator, f: &mut impl FnMut(Local)) {
    match t {
        Terminator::If { cond, .. } => operand_reads(cond, f),
        Terminator::Switch { value, .. } => operand_reads(value, f),
        Terminator::Return(Some(o)) | Terminator::AsyncComplete(Some(o)) => operand_reads(o, f),
        Terminator::Await { future, .. } => operand_reads(future, f),
        Terminator::TailCall { args, .. } => args.iter().for_each(|a| operand_reads(a, f)),
        _ => {}
    }
}

fn operand_reads(op: &Operand, f: &mut impl FnMut(Local)) {
    if let Operand::Copy(place) = op {
        match place {
            Place::Local(l) => f(*l),
            Place::Field { base, .. } => f(*base),
            Place::Index { base, index } => {
                f(*base);
                operand_reads(index, f);
            }
            Place::Global(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Const, Rvalue};

    /// entry -> cond -> (body: t = 2 + 3; goto cond | after: return 0). `t` is loop-invariant, used
    /// nowhere, so LICM must hoist `t = 2 + 3` out of `body` into a preheader.
    #[test]
    fn hoists_invariant_out_of_loop() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let t = b.new_temp(i.int());
        let cond = b.new_block();
        let body = b.new_block();
        let after = b.new_block();
        b.terminate(Terminator::Goto(cond));
        b.switch_to(cond);
        b.terminate(Terminator::If {
            cond: Operand::Const(Const::Bool(true)),
            then_blk: body,
            else_blk: after,
        });
        b.switch_to(body);
        b.assign(
            Place::Local(t),
            Rvalue::Binary(
                BinOp::Add,
                Operand::Const(Const::Int(2)),
                Operand::Const(Const::Int(3)),
            ),
        );
        b.terminate(Terminator::Goto(cond));
        b.switch_to(after);
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Int(0)))));
        let mut func = b.finish();

        assert!(Licm.run(&mut func, &i), "should hoist");
        // The body block (index 2) must no longer contain the assignment.
        let body_has_assign = func.blocks[2]
            .stmts
            .iter()
            .any(|s| matches!(s, Statement::Assign(Place::Local(l), _) if *l == t));
        assert!(!body_has_assign, "assignment left in loop body");
        // A new preheader block holds it.
        let hoisted_somewhere = func
            .blocks
            .iter()
            .any(|bb| bb.stmts.iter().any(|s| matches!(s, Statement::Assign(Place::Local(l), _) if *l == t)));
        assert!(hoisted_somewhere, "assignment vanished entirely");
    }

    /// A loop-carried accumulator (`s = s + 1`) reads its own destination, so it is *not* invariant
    /// and must stay in the loop.
    #[test]
    fn does_not_hoist_loop_carried() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let s = b.new_temp(i.int());
        let cond = b.new_block();
        let body = b.new_block();
        let after = b.new_block();
        b.assign(Place::Local(s), Rvalue::Use(Operand::Const(Const::Int(0))));
        b.terminate(Terminator::Goto(cond));
        b.switch_to(cond);
        b.terminate(Terminator::If {
            cond: Operand::Const(Const::Bool(true)),
            then_blk: body,
            else_blk: after,
        });
        b.switch_to(body);
        b.assign(
            Place::Local(s),
            Rvalue::Binary(
                BinOp::Add,
                Operand::Copy(Place::Local(s)),
                Operand::Const(Const::Int(1)),
            ),
        );
        b.terminate(Terminator::Goto(cond));
        b.switch_to(after);
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(s)))));
        let mut func = b.finish();

        assert!(!Licm.run(&mut func, &i), "must not hoist a loop-carried value");
    }
}
