//! Relooper: reconstruct structured control flow (WASM `block`/`loop`/`if`) from the MIR CFG.
//!
//! Dream's surface language only produces reducible CFGs, so the classic Relooper shapes suffice:
//! [`Shape::Simple`] (a basic block followed by the structured remainder), [`Shape::Loop`] (a
//! cyclic region wrapped in a `loop`), and [`Shape::Multiple`] (independent branch arms joined by a
//! following region). The backend ([`super::emit`]) walks this tree to place `block`/`loop` scopes
//! and turn CFG edges into `br`/`br_if`.

use super::{BlockId, MirFunction};
use std::collections::{BTreeSet, VecDeque};

/// A structured control-flow tree node.
#[derive(Debug, PartialEq, Eq)]
pub enum Shape {
    /// A single block, then the structured remainder.
    Simple {
        block: BlockId,
        next: Option<Box<Shape>>,
    },
    /// A loop body (`inner`) wrapped so back-edges branch to its top, then the post-loop remainder.
    Loop {
        inner: Box<Shape>,
        next: Option<Box<Shape>>,
    },
    /// Independent arms (each with its own entry), then the join remainder.
    Multiple {
        handled: Vec<Shape>,
        next: Option<Box<Shape>>,
    },
}

/// Builds the structured shape for `func`'s body starting at its entry block.
pub fn reloop(func: &MirFunction) -> Option<Shape> {
    let r = Relooper { func };
    let all: BTreeSet<BlockId> = (0..func.blocks.len() as u32).map(BlockId).collect();
    r.make(singleton(func.entry), all, &BTreeSet::new())
        .map(|b| *b)
}

struct Relooper<'a> {
    func: &'a MirFunction,
}

impl Relooper<'_> {
    /// Forward successors of `b`, excluding edges back to an enclosing loop's headers (those are
    /// `continue` edges handled by the surrounding [`Shape::Loop`], not forward control flow).
    fn succs(&self, b: BlockId, headers: &BTreeSet<BlockId>) -> Vec<BlockId> {
        self.func
            .block(b)
            .terminator
            .successors()
            .into_iter()
            .filter(|s| !headers.contains(s))
            .collect()
    }

    /// All blocks reachable from `entries`, staying within `within`, not traversing back into
    /// enclosing-loop `headers`.
    fn reach(
        &self,
        entries: &BTreeSet<BlockId>,
        within: &BTreeSet<BlockId>,
        headers: &BTreeSet<BlockId>,
    ) -> BTreeSet<BlockId> {
        let mut seen = BTreeSet::new();
        let mut queue: VecDeque<BlockId> = entries.iter().copied().collect();
        while let Some(b) = queue.pop_front() {
            if !within.contains(&b) || !seen.insert(b) {
                continue;
            }
            for s in self.succs(b, headers) {
                if within.contains(&s) {
                    queue.push_back(s);
                }
            }
        }
        seen
    }

    /// True if some reachable block branches back to `entry` (ignoring enclosing-loop back-edges),
    /// making `entry` a loop header at this level.
    fn has_back_edge(
        &self,
        entry: BlockId,
        within: &BTreeSet<BlockId>,
        headers: &BTreeSet<BlockId>,
    ) -> bool {
        let reachable = self.reach(&singleton(entry), within, headers);
        reachable
            .iter()
            .any(|&b| self.succs(b, headers).contains(&entry))
    }

    fn make(
        &self,
        entries: BTreeSet<BlockId>,
        remaining: BTreeSet<BlockId>,
        headers: &BTreeSet<BlockId>,
    ) -> Option<Box<Shape>> {
        let entries: BTreeSet<BlockId> = entries
            .into_iter()
            .filter(|e| remaining.contains(e))
            .collect();
        if entries.is_empty() {
            return None;
        }

        if entries.len() == 1 {
            let e = *entries.iter().next().unwrap();
            if !self.has_back_edge(e, &remaining, headers) {
                return Some(self.make_simple(e, remaining, headers));
            }
            return Some(self.make_loop(entries, remaining, headers));
        }

        let is_loop = entries
            .iter()
            .any(|&e| self.has_back_edge(e, &remaining, headers));
        if is_loop {
            Some(self.make_loop(entries, remaining, headers))
        } else {
            Some(self.make_multiple(entries, remaining, headers))
        }
    }

    fn make_simple(
        &self,
        e: BlockId,
        remaining: BTreeSet<BlockId>,
        headers: &BTreeSet<BlockId>,
    ) -> Box<Shape> {
        let mut next_remaining = remaining;
        next_remaining.remove(&e);
        let next_entries: BTreeSet<BlockId> = self
            .succs(e, headers)
            .into_iter()
            .filter(|s| next_remaining.contains(s))
            .collect();
        let next = self.make(next_entries, next_remaining, headers);
        Box::new(Shape::Simple { block: e, next })
    }

    fn make_loop(
        &self,
        entries: BTreeSet<BlockId>,
        remaining: BTreeSet<BlockId>,
        headers: &BTreeSet<BlockId>,
    ) -> Box<Shape> {
        let reachable = self.reach(&entries, &remaining, headers);
        let inner: BTreeSet<BlockId> = reachable
            .iter()
            .copied()
            .filter(|&b| self.can_reach_any(b, &entries, &remaining, headers))
            .collect();

        // Inside the loop body, the loop's own entries become headers (their back-edges are
        // `continue`s, not new loops).
        let inner_headers: BTreeSet<BlockId> = headers.union(&entries).copied().collect();

        let mut next_entries: BTreeSet<BlockId> = BTreeSet::new();
        for &b in &inner {
            for s in self.succs(b, headers) {
                if remaining.contains(&s) && !inner.contains(&s) {
                    next_entries.insert(s);
                }
            }
        }
        let outer: BTreeSet<BlockId> = remaining.difference(&inner).copied().collect();

        let inner_shape = self
            .make(entries, inner, &inner_headers)
            .expect("loop body is non-empty");
        let next = self.make(next_entries, outer, headers);
        Box::new(Shape::Loop {
            inner: inner_shape,
            next,
        })
    }

    fn make_multiple(
        &self,
        entries: BTreeSet<BlockId>,
        remaining: BTreeSet<BlockId>,
        headers: &BTreeSet<BlockId>,
    ) -> Box<Shape> {
        let mut count: std::collections::BTreeMap<BlockId, usize> =
            std::collections::BTreeMap::new();
        let mut per_entry: Vec<(BlockId, BTreeSet<BlockId>)> = Vec::new();
        for &e in &entries {
            let r = self.reach(&singleton(e), &remaining, headers);
            for &b in &r {
                *count.entry(b).or_insert(0) += 1;
            }
            per_entry.push((e, r));
        }
        let join: BTreeSet<BlockId> = count
            .iter()
            .filter(|(_, &c)| c >= 2)
            .map(|(&b, _)| b)
            .collect();

        let mut handled = Vec::new();
        let mut consumed: BTreeSet<BlockId> = BTreeSet::new();
        for (e, reach) in &per_entry {
            if join.contains(e) {
                continue;
            }
            let group: BTreeSet<BlockId> = reach
                .iter()
                .copied()
                .filter(|b| !join.contains(b))
                .collect();
            consumed.extend(group.iter().copied());
            if let Some(shape) = self.make(singleton(*e), group, headers) {
                handled.push(*shape);
            }
        }

        let outer: BTreeSet<BlockId> = remaining.difference(&consumed).copied().collect();
        let next = self.make(join, outer, headers);
        Box::new(Shape::Multiple { handled, next })
    }

    fn can_reach_any(
        &self,
        b: BlockId,
        targets: &BTreeSet<BlockId>,
        within: &BTreeSet<BlockId>,
        headers: &BTreeSet<BlockId>,
    ) -> bool {
        let r = self.reach(&singleton(b), within, headers);
        targets.iter().any(|t| r.contains(t))
    }
}

fn singleton(b: BlockId) -> BTreeSet<BlockId> {
    let mut s = BTreeSet::new();
    s.insert(b);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Const, Operand, Terminator};
    use crate::types::TypeInterner;

    fn ret(v: i64) -> Terminator {
        Terminator::Return(Some(Operand::Const(Const::Int(v))))
    }

    #[test]
    fn linear_chain_is_nested_simple() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let b1 = b.new_block();
        b.terminate(Terminator::Goto(b1));
        b.switch_to(b1);
        b.terminate(ret(0));
        let func = b.finish();

        let shape = reloop(&func).unwrap();
        match shape {
            Shape::Simple { block, next } => {
                assert_eq!(block, BlockId(0));
                assert!(
                    matches!(next.as_deref(), Some(Shape::Simple { block, next: None }) if *block == b1)
                );
            }
            other => panic!("expected nested simple, got {:?}", other),
        }
    }

    #[test]
    fn if_diamond_is_simple_then_multiple() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let then_blk = b.new_block();
        let else_blk = b.new_block();
        let join = b.new_block();
        b.terminate(Terminator::If {
            cond: Operand::Const(Const::Bool(true)),
            then_blk,
            else_blk,
        });
        b.switch_to(then_blk);
        b.terminate(Terminator::Goto(join));
        b.switch_to(else_blk);
        b.terminate(Terminator::Goto(join));
        b.switch_to(join);
        b.terminate(ret(0));
        let func = b.finish();

        let shape = reloop(&func).unwrap();
        // entry is Simple, its next is a Multiple with two arms then the join.
        match shape {
            Shape::Simple { block, next } => {
                assert_eq!(block, BlockId(0));
                match next.as_deref() {
                    Some(Shape::Multiple { handled, next }) => {
                        assert_eq!(handled.len(), 2);
                        assert!(
                            matches!(next.as_deref(), Some(Shape::Simple { block, .. }) if *block == join)
                        );
                    }
                    other => panic!("expected multiple, got {:?}", other),
                }
            }
            other => panic!("expected simple entry, got {:?}", other),
        }
    }

    #[test]
    fn while_loop_is_loop_shape() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
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
        b.terminate(Terminator::Goto(cond)); // back edge
        b.switch_to(after);
        b.terminate(ret(0));
        let func = b.finish();

        let shape = reloop(&func).unwrap();
        // entry (bb0) is Simple, then a Loop (cond+body), then Simple(after).
        match shape {
            Shape::Simple { block, next } => {
                assert_eq!(block, BlockId(0));
                match next.as_deref() {
                    Some(Shape::Loop { next, .. }) => {
                        assert!(
                            matches!(next.as_deref(), Some(Shape::Simple { block, .. }) if *block == after)
                        );
                    }
                    other => panic!("expected loop, got {:?}", other),
                }
            }
            other => panic!("expected simple entry, got {:?}", other),
        }
    }
}
