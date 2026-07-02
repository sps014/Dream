//! Intra-block common-subexpression elimination for pure integer/boolean arithmetic. Within a single
//! basic block, a `Binary`/`Unary` computation with the same operator and operands as an earlier one
//! (whose result local has not been overwritten since) is replaced by a copy of that earlier result.
//! Copy/constant propagation and DCE then clean up the redundant copy.
//!
//! Restricted to `Binary`/`Unary` over constants and local reads: these never touch memory, so no
//! call or store between the two occurrences can invalidate the value. The only way a cached
//! expression becomes stale is a reassignment of one of its operand locals (or its own result
//! local), which is handled by invalidation.

use super::MirPass;
use crate::mir::{BinOp, Const, Local, MirFunction, Operand, Place, Rvalue, Statement, UnOp};
use crate::types::TypeInterner;

pub struct Gvn;

/// A canonical, hashable key for a redundancy-eligible expression.
#[derive(PartialEq, Eq, Hash, Clone)]
enum Key {
    Binary(BinOp, OpKey, OpKey),
    Unary(UnOp, OpKey),
}

#[derive(PartialEq, Eq, Hash, Clone)]
enum OpKey {
    Local(u32),
    Int(i64),
    Long(i64),
    Bool(bool),
    Char(char),
    /// Any operand shape we do not want to number (floats: bit patterns are not `Eq`/`Hash`; memory
    /// reads through field/index places). Never matches another key, disabling CSE for it.
    Opaque,
}

impl MirPass for Gvn {
    fn name(&self) -> &'static str {
        "gvn"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = false;
        for block in &mut func.blocks {
            // Available expressions: key -> the local currently holding its value.
            let mut avail: Vec<(Key, u32)> = Vec::new();
            for stmt in &mut block.stmts {
                if let Statement::Assign(Place::Local(dest), rvalue) = stmt {
                    let dest_id = dest.0;
                    if let Some(key) = key_of(rvalue) {
                        if let Some(&(_, src)) =
                            avail.iter().find(|(k, l)| *k == key && *l != dest_id)
                        {
                            *rvalue = Rvalue::Use(Operand::Copy(Place::Local(Local(src))));
                            changed = true;
                            // The value in `dest` is now just a copy; still record it below so a third
                            // occurrence can reuse either. Fall through to invalidation + insert.
                        }
                        invalidate(&mut avail, dest_id);
                        avail.push((key, dest_id));
                    } else {
                        // Impure / un-numbered rvalue still defines `dest`: drop stale entries.
                        invalidate(&mut avail, dest_id);
                    }
                } else if let Statement::Assign(_, _) = stmt {
                    // Store through a non-local place: no local is defined, and numbered expressions
                    // depend only on locals/consts, so nothing is invalidated.
                }
                // Retain/Release/Call/InterfaceCall/Print define no local and cannot affect a numbered
                // (memory-free) expression, so they need no invalidation.
            }
        }
        changed
    }
}

/// Drops every available entry defined into `dest` or reading `dest` as an operand (its value is now
/// stale).
fn invalidate(avail: &mut Vec<(Key, u32)>, dest: u32) {
    avail.retain(|(k, l)| *l != dest && !key_mentions(k, dest));
}

fn key_mentions(k: &Key, local: u32) -> bool {
    let mentions = |o: &OpKey| matches!(o, OpKey::Local(l) if *l == local);
    match k {
        Key::Binary(_, a, b) => mentions(a) || mentions(b),
        Key::Unary(_, a) => mentions(a),
    }
}

fn key_of(rvalue: &Rvalue) -> Option<Key> {
    match rvalue {
        Rvalue::Binary(op, a, b) => Some(Key::Binary(*op, op_key(a)?, op_key(b)?)),
        Rvalue::Unary(op, a) => Some(Key::Unary(*op, op_key(a)?)),
        _ => None,
    }
}

/// A hashable key for an operand, or `None` for shapes we refuse to number (currently none — floats
/// map to [`OpKey::Opaque`] so they never match).
fn op_key(op: &Operand) -> Option<OpKey> {
    Some(match op {
        Operand::Copy(Place::Local(l)) => OpKey::Local(l.0),
        Operand::Copy(_) => OpKey::Opaque, // field/index/global read: may alias memory
        Operand::Const(Const::Int(v)) => OpKey::Int(*v),
        Operand::Const(Const::Long(v)) => OpKey::Long(*v),
        Operand::Const(Const::Bool(b)) => OpKey::Bool(*b),
        Operand::Const(Const::Char(c)) => OpKey::Char(*c),
        Operand::Const(_) => OpKey::Opaque, // floats/strings/null
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Operand, Place, Rvalue, Terminator};

    #[test]
    fn dedups_repeated_binary() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let x = b.new_param(i.int(), Some("x".into()));
        let a = b.new_temp(i.int());
        let c = b.new_temp(i.int());
        let mul = || {
            Rvalue::Binary(
                BinOp::Mul,
                Operand::Copy(Place::Local(x)),
                Operand::Const(Const::Int(4)),
            )
        };
        b.assign(Place::Local(a), mul());
        b.assign(Place::Local(c), mul());
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(c)))));
        let mut func = b.finish();
        assert!(Gvn.run(&mut func, &i));
        // Second `x*4` becomes `Use(a)`.
        match &func.blocks[0].stmts[1] {
            Statement::Assign(_, Rvalue::Use(Operand::Copy(Place::Local(l)))) => assert_eq!(*l, a),
            other => panic!("expected CSE copy, got {:?}", other),
        }
    }

    #[test]
    fn invalidates_on_operand_redef() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let x = b.new_local(i.int(), Some("x".into()));
        let a = b.new_temp(i.int());
        let c = b.new_temp(i.int());
        b.assign(
            Place::Local(a),
            Rvalue::Binary(
                BinOp::Add,
                Operand::Copy(Place::Local(x)),
                Operand::Const(Const::Int(1)),
            ),
        );
        // Redefine x between the two adds: the second must NOT be CSE'd.
        b.assign(Place::Local(x), Rvalue::Use(Operand::Const(Const::Int(9))));
        b.assign(
            Place::Local(c),
            Rvalue::Binary(
                BinOp::Add,
                Operand::Copy(Place::Local(x)),
                Operand::Const(Const::Int(1)),
            ),
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(c)))));
        let mut func = b.finish();
        Gvn.run(&mut func, &i);
        assert!(matches!(
            &func.blocks[0].stmts[2],
            Statement::Assign(_, Rvalue::Binary(..))
        ));
    }
}
