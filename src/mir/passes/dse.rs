//! Intra-block dead-store elimination. A store into memory (`obj.field = …`, `arr[i] = …`, or a
//! module global) is dead if a later store in the same block overwrites the *same* location before
//! anything can read it. Such a store is replaced with `Nop` (DCE clears it).
//!
//! Aliasing is handled conservatively: any statement that could read or observe memory — a load,
//! call, print, retain/release, or a store whose location we cannot pin down — is a barrier that
//! forgets all pending stores. A store is only killed by an overwrite to a *syntactically identical*
//! location with no barrier in between, and only when its own right-hand side is pure (so deleting
//! it drops no side effect). Stores still pending at the block end are kept (they may be read in a
//! successor).

use super::dce::is_pure;
use super::MirPass;
use crate::mir::{Const, Global, Local, MirFunction, Operand, Place, Rvalue, Statement};
use crate::types::TypeInterner;
use std::collections::HashMap;

pub struct Dse;

impl MirPass for Dse {
    fn name(&self) -> &'static str {
        "dse"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = false;
        for block in &mut func.blocks {
            let mut pending: HashMap<PKey, usize> = HashMap::new();
            let mut dead: Vec<usize> = Vec::new();
            for (idx, stmt) in block.stmts.iter().enumerate() {
                match stmt {
                    Statement::Assign(place, rv) => {
                        // The value side may read memory (a load) or have effects; if so it is a
                        // barrier that happens *before* this store completes.
                        if rvalue_touches_memory(rv) {
                            pending.clear();
                        }
                        // Overwriting a base/global local invalidates pending stores keyed on it.
                        if let Place::Local(l) = place {
                            pending.retain(|k, _| k.base_local() != Some(*l));
                        }
                        match place_key(place) {
                            Some(key) => {
                                if let Some(prev) = pending.get(&key).copied() {
                                    if is_pure(stmt_rvalue(&block.stmts[prev])) {
                                        dead.push(prev);
                                    }
                                }
                                pending.insert(key, idx);
                            }
                            // A store we can't key (e.g. a non-constant, non-local array index) may
                            // alias anything: barrier.
                            None if is_memory_place(place) => pending.clear(),
                            None => {}
                        }
                    }
                    // Anything that can observe memory forgets all pending stores.
                    Statement::Call { .. }
                    | Statement::InterfaceCall { .. }
                    | Statement::Print { .. }
                    | Statement::Retain(_)
                    | Statement::Release(_) => pending.clear(),
                    // A debug line-hook is an observable host call: it must see every prior store, so
                    // it forgets all pending (still-eliminable) stores.
                    Statement::DebugLine(_) => pending.clear(),
                    Statement::Nop => {}
                }
            }
            if !dead.is_empty() {
                for idx in dead {
                    block.stmts[idx] = Statement::Nop;
                }
                changed = true;
            }
        }
        changed
    }
}

/// A canonical key for a memory location we can prove two stores share. `None`-keyed places are
/// treated as may-alias barriers by the caller.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
enum PKey {
    Field(Local, usize),
    Global(Global),
    IndexConst(Local, i64),
    IndexLocal(Local, Local),
}

impl PKey {
    /// The base local this key is anchored on (if any), for invalidation when that local is
    /// reassigned.
    fn base_local(&self) -> Option<Local> {
        match self {
            PKey::Field(b, _) | PKey::IndexConst(b, _) | PKey::IndexLocal(b, _) => Some(*b),
            PKey::Global(_) => None,
        }
    }
}

fn place_key(place: &Place) -> Option<PKey> {
    match place {
        Place::Field { base, field } => Some(PKey::Field(*base, *field)),
        Place::Global(g) => Some(PKey::Global(*g)),
        Place::Index { base, index } => match index.as_ref() {
            Operand::Const(Const::Int(v)) | Operand::Const(Const::Long(v)) => {
                Some(PKey::IndexConst(*base, *v))
            }
            Operand::Copy(Place::Local(l)) => Some(PKey::IndexLocal(*base, *l)),
            _ => None,
        },
        Place::Local(_) => None,
    }
}

fn is_memory_place(place: &Place) -> bool {
    matches!(
        place,
        Place::Field { .. } | Place::Index { .. } | Place::Global(_)
    )
}

fn stmt_rvalue(stmt: &Statement) -> &Rvalue {
    match stmt {
        Statement::Assign(_, rv) => rv,
        _ => unreachable!("pending store is always an Assign"),
    }
}

/// True if evaluating this rvalue could load from or otherwise observe memory (so it must act as a
/// barrier). Only pure register computations over constants/locals are memory-free.
fn rvalue_touches_memory(rv: &Rvalue) -> bool {
    match rv {
        Rvalue::Use(o) | Rvalue::Cast(o, _, _) => operand_touches_memory(o),
        Rvalue::Binary(_, a, b) => operand_touches_memory(a) || operand_touches_memory(b),
        Rvalue::Unary(_, a) => operand_touches_memory(a),
        Rvalue::FuncRef(_) => false,
        _ => true,
    }
}

fn operand_touches_memory(op: &Operand) -> bool {
    matches!(
        op,
        Operand::Copy(Place::Field { .. }) | Operand::Copy(Place::Index { .. })
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Operand, Place, Rvalue, Terminator};

    #[test]
    fn kills_overwritten_field_store() {
        // obj.0 = 1; obj.0 = 2;  ->  first store is dead.
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let obj = b.new_temp(i.int());
        b.assign(
            Place::Field { base: obj, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(1))),
        );
        b.assign(
            Place::Field { base: obj, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(2))),
        );
        b.terminate(Terminator::Return(None));
        let mut func = b.finish();
        assert!(Dse.run(&mut func, &i));
        assert!(
            matches!(func.blocks[0].stmts[0], Statement::Nop),
            "first (overwritten) store should be dead"
        );
        assert!(matches!(func.blocks[0].stmts[1], Statement::Assign(..)));
    }

    #[test]
    fn keeps_store_read_before_overwrite() {
        // obj.0 = 1; x = obj.0; obj.0 = 2;  ->  first store is observed, must stay.
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let obj = b.new_temp(i.int());
        let x = b.new_temp(i.int());
        b.assign(
            Place::Field { base: obj, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(1))),
        );
        b.assign(
            Place::Local(x),
            Rvalue::Use(Operand::Copy(Place::Field { base: obj, field: 0 })),
        );
        b.assign(
            Place::Field { base: obj, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(2))),
        );
        b.terminate(Terminator::Return(None));
        let mut func = b.finish();
        assert!(!Dse.run(&mut func, &i), "store observed by a load must be kept");
    }

    #[test]
    fn call_between_stores_is_a_barrier() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let obj = b.new_temp(i.int());
        b.assign(
            Place::Field { base: obj, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(1))),
        );
        b.push(Statement::Print {
            arg: Operand::Const(Const::Int(0)),
            ty: i.int(),
            newline: false,
        });
        b.assign(
            Place::Field { base: obj, field: 0 },
            Rvalue::Use(Operand::Const(Const::Int(2))),
        );
        b.terminate(Terminator::Return(None));
        let mut func = b.finish();
        assert!(!Dse.run(&mut func, &i), "a print between stores is a barrier");
    }
}
