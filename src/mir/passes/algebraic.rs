//! Algebraic identities and strength reduction on integer arithmetic. Rewrites redundant operations
//! (`x + 0`, `x * 1`, `x * 0`, …) to a plain use and turns multiplication by a power of two into a
//! left shift. Restricted to integer (`Int`/`Long`) constants: float rewrites are skipped to avoid
//! signed-zero / NaN pitfalls, and signed division-by-shift is unsound so it is not attempted.

use super::MirPass;
use crate::mir::{BinOp, Const, MirFunction, Operand, Rvalue, Statement};
use crate::types::TypeInterner;

pub struct Algebraic;

impl MirPass for Algebraic {
    fn name(&self) -> &'static str {
        "algebraic"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = false;
        for block in &mut func.blocks {
            for stmt in &mut block.stmts {
                if let Statement::Assign(_, rvalue) = stmt {
                    if let Some(simpler) = simplify(rvalue) {
                        *rvalue = simpler;
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

/// The integer value of a constant operand, if it is an `Int`/`Long`.
fn int_val(op: &Operand) -> Option<i64> {
    match op {
        Operand::Const(Const::Int(v)) | Operand::Const(Const::Long(v)) => Some(*v),
        _ => None,
    }
}

fn is_int_const(op: &Operand) -> bool {
    matches!(op, Operand::Const(Const::Int(_) | Const::Long(_)))
}

/// A shift-amount constant matching the width of `factor` (the constant being strength-reduced), so
/// the emitted `Shl` stays type-correct for `i64` operands.
fn shift_const(factor: &Operand, k: i64) -> Operand {
    match factor {
        Operand::Const(Const::Long(_)) => Operand::Const(Const::Long(k)),
        _ => Operand::Const(Const::Int(k)),
    }
}

/// Returns `Some(k)` if `v == 2^k` for `k >= 1` (so `* v` becomes `<< k`).
fn log2_pow2(v: i64) -> Option<i64> {
    if v > 1 && (v & (v - 1)) == 0 {
        Some(v.trailing_zeros() as i64)
    } else {
        None
    }
}

fn simplify(rvalue: &Rvalue) -> Option<Rvalue> {
    let Rvalue::Binary(op, a, b) = rvalue else {
        return None;
    };
    let ac = int_val(a);
    let bc = int_val(b);
    match op {
        BinOp::Add => {
            if bc == Some(0) {
                return Some(Rvalue::Use(a.clone())); // x + 0
            }
            if ac == Some(0) {
                return Some(Rvalue::Use(b.clone())); // 0 + x
            }
        }
        BinOp::Sub => {
            if bc == Some(0) {
                return Some(Rvalue::Use(a.clone())); // x - 0
            }
        }
        BinOp::Mul => {
            if bc == Some(0) && is_int_const(b) {
                return Some(Rvalue::Use(b.clone())); // x * 0 -> 0
            }
            if ac == Some(0) && is_int_const(a) {
                return Some(Rvalue::Use(a.clone())); // 0 * x -> 0
            }
            if bc == Some(1) {
                return Some(Rvalue::Use(a.clone())); // x * 1
            }
            if ac == Some(1) {
                return Some(Rvalue::Use(b.clone())); // 1 * x
            }
            // Strength reduction: x * 2^k -> x << k. The shift amount must keep the multiplied
            // operand's width (an `i64` shift needs an `i64` amount), so mirror the constant factor's
            // `Int`/`Long` kind rather than always emitting an `i32`.
            if let Some(k) = bc.and_then(log2_pow2) {
                return Some(Rvalue::Binary(BinOp::Shl, a.clone(), shift_const(b, k)));
            }
            if let Some(k) = ac.and_then(log2_pow2) {
                return Some(Rvalue::Binary(BinOp::Shl, b.clone(), shift_const(a, k)));
            }
        }
        BinOp::Div => {
            if bc == Some(1) {
                return Some(Rvalue::Use(a.clone())); // x / 1
            }
        }
        BinOp::BitOr => {
            if bc == Some(0) {
                return Some(Rvalue::Use(a.clone())); // x | 0
            }
            if ac == Some(0) {
                return Some(Rvalue::Use(b.clone())); // 0 | x
            }
        }
        BinOp::BitXor => {
            if bc == Some(0) {
                return Some(Rvalue::Use(a.clone())); // x ^ 0
            }
            if ac == Some(0) {
                return Some(Rvalue::Use(b.clone())); // 0 ^ x
            }
        }
        BinOp::BitAnd => {
            if bc == Some(0) && is_int_const(b) {
                return Some(Rvalue::Use(b.clone())); // x & 0 -> 0
            }
            if ac == Some(0) && is_int_const(a) {
                return Some(Rvalue::Use(a.clone())); // 0 & x -> 0
            }
        }
        BinOp::Shl | BinOp::Shr
            if bc == Some(0) => {
                return Some(Rvalue::Use(a.clone())); // x << 0 / x >> 0
            }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Operand, Place, Rvalue, Terminator};

    fn run_one(rv: Rvalue) -> Rvalue {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let t = b.new_temp(i.int());
        b.assign(Place::Local(t), rv);
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));
        let mut func = b.finish();
        Algebraic.run(&mut func, &i);
        match &func.blocks[0].stmts[0] {
            Statement::Assign(_, r) => r.clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn add_zero_is_use() {
        let x = Operand::Copy(Place::Local(crate::mir::Local(0)));
        let r = run_one(Rvalue::Binary(
            BinOp::Add,
            x.clone(),
            Operand::Const(Const::Int(0)),
        ));
        assert!(matches!(r, Rvalue::Use(Operand::Copy(_))));
    }

    #[test]
    fn mul_pow2_is_shift() {
        let x = Operand::Copy(Place::Local(crate::mir::Local(0)));
        let r = run_one(Rvalue::Binary(BinOp::Mul, x, Operand::Const(Const::Int(8))));
        match r {
            Rvalue::Binary(BinOp::Shl, _, Operand::Const(Const::Int(k))) => assert_eq!(k, 3),
            other => panic!("expected shift, got {:?}", other),
        }
    }

    #[test]
    fn mul_zero_is_zero() {
        let x = Operand::Copy(Place::Local(crate::mir::Local(0)));
        let r = run_one(Rvalue::Binary(BinOp::Mul, x, Operand::Const(Const::Int(0))));
        assert!(matches!(r, Rvalue::Use(Operand::Const(Const::Int(0)))));
    }
}
