//! Algebraic identities and strength reduction on integer arithmetic. Rewrites redundant operations
//! (`x + 0`, `x * 1`, `x * 0`, …) to a plain use and turns multiplication by a power of two into a
//! left shift. Restricted to integer (`Int`/`Long`) constants: float rewrites are skipped to avoid
//! signed-zero / NaN pitfalls, and signed division-by-shift is unsound so it is not attempted.

use super::MirPass;
use crate::mir::{BinOp, Const, MirFunction, Operand, Place, Rvalue, Statement};
use crate::types::{PrimTy, TyKind, TypeId, TypeInterner};

pub struct Algebraic;

impl MirPass for Algebraic {
    fn name(&self) -> &'static str {
        "algebraic"
    }

    fn run(&self, func: &mut MirFunction, interner: &TypeInterner) -> bool {
        // Snapshot local types so the mutable walk of `func.blocks` below doesn't re-borrow `func`.
        let local_tys: Vec<TypeId> = func.locals.iter().map(|d| d.ty).collect();
        let mut changed = false;
        for block in &mut func.blocks {
            for stmt in &mut block.stmts {
                if let Statement::Assign(place, rvalue) = stmt {
                    // The destination's scalar type disambiguates signed vs. unsigned strength
                    // reduction (only unsigned `/`,`%` by a power of two are sound as shift/mask).
                    let dest_ty = match place {
                        Place::Local(l) => Some(local_tys[l.0 as usize]),
                        _ => None,
                    };
                    // A floating-point operation can still carry an un-widened integer constant
                    // operand (e.g. `someDouble * 4`, where the `4` is an `Int` const the backend
                    // widens to `f64`). The integer identities/strength reduction here would then
                    // wrongly fold it (`* 4` -> `<< 2`), producing a nonsensical float shift. Skip
                    // them entirely for float/double results and let the backend do the widening.
                    if dest_ty.map(|t| is_float(interner, t)).unwrap_or(false) {
                        continue;
                    }
                    let unsigned = dest_ty.map(|t| is_unsigned(interner, t)).unwrap_or(false);
                    if let Some(simpler) = simplify(rvalue, unsigned) {
                        *rvalue = simpler;
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

/// True for the unsigned integer primitives, whose `/` and `%` map to `shr_u` / `and`.
fn is_unsigned(interner: &TypeInterner, ty: TypeId) -> bool {
    matches!(
        interner.kind(interner.strip_nullable(ty)),
        TyKind::Prim(PrimTy::UInt | PrimTy::ULong | PrimTy::Byte)
    )
}

/// True for the floating-point primitives, whose arithmetic must never be strength-reduced to
/// integer shifts/masks (see the guard in [`Algebraic::run`]).
fn is_float(interner: &TypeInterner, ty: TypeId) -> bool {
    matches!(
        interner.kind(interner.strip_nullable(ty)),
        TyKind::Prim(PrimTy::Float | PrimTy::Double)
    )
}

/// The bool value of a constant operand, if any.
fn bool_val(op: &Operand) -> Option<bool> {
    match op {
        Operand::Const(Const::Bool(b)) => Some(*b),
        _ => None,
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

/// `2^k - 1` mask for an unsigned `% 2^k`, matching the operand width.
fn mask_const(operand: &Operand, k: i64) -> Operand {
    let m = (1i64 << k) - 1;
    match operand {
        Operand::Const(Const::Long(_)) => Operand::Const(Const::Long(m)),
        _ => Operand::Const(Const::Int(m)),
    }
}

fn simplify(rvalue: &Rvalue, unsigned: bool) -> Option<Rvalue> {
    let Rvalue::Binary(op, a, b) = rvalue else {
        return None;
    };
    let ac = int_val(a);
    let bc = int_val(b);
    match op {
        // Boolean identities against a constant (feeds SimplifyCfg's branch folding).
        BinOp::And if bool_val(b) == Some(true) => return Some(Rvalue::Use(a.clone())),
        BinOp::And if bool_val(a) == Some(true) => return Some(Rvalue::Use(b.clone())),
        BinOp::And if bool_val(b) == Some(false) || bool_val(a) == Some(false) => {
            return Some(Rvalue::Use(Operand::Const(Const::Bool(false))));
        }
        BinOp::Or if bool_val(b) == Some(false) => return Some(Rvalue::Use(a.clone())),
        BinOp::Or if bool_val(a) == Some(false) => return Some(Rvalue::Use(b.clone())),
        BinOp::Or if bool_val(b) == Some(true) || bool_val(a) == Some(true) => {
            return Some(Rvalue::Use(Operand::Const(Const::Bool(true))));
        }
        // `x == true`/`x != false` -> x ; `x == false`/`x != true` -> !x.
        BinOp::Eq if bool_val(b) == Some(true) => return Some(Rvalue::Use(a.clone())),
        BinOp::Eq if bool_val(a) == Some(true) => return Some(Rvalue::Use(b.clone())),
        BinOp::Ne if bool_val(b) == Some(false) => return Some(Rvalue::Use(a.clone())),
        BinOp::Ne if bool_val(a) == Some(false) => return Some(Rvalue::Use(b.clone())),
        BinOp::Eq if bool_val(b) == Some(false) => return Some(Rvalue::Unary(crate::mir::UnOp::Not, a.clone())),
        BinOp::Eq if bool_val(a) == Some(false) => return Some(Rvalue::Unary(crate::mir::UnOp::Not, b.clone())),
        BinOp::Ne if bool_val(b) == Some(true) => return Some(Rvalue::Unary(crate::mir::UnOp::Not, a.clone())),
        BinOp::Ne if bool_val(a) == Some(true) => return Some(Rvalue::Unary(crate::mir::UnOp::Not, b.clone())),
        _ => {}
    }
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
            // Unsigned x / 2^k -> x >> k (emits `shr_u` for the unsigned destination). Signed
            // division rounds toward zero, so a shift would be wrong for negatives — skip it.
            if unsigned {
                if let Some(k) = bc.and_then(log2_pow2) {
                    return Some(Rvalue::Binary(BinOp::Shr, a.clone(), shift_const(b, k)));
                }
            }
        }
        BinOp::Rem => {
            // Unsigned x % 2^k -> x & (2^k - 1).
            if unsigned {
                if let Some(k) = bc.and_then(log2_pow2) {
                    return Some(Rvalue::Binary(BinOp::BitAnd, a.clone(), mask_const(b, k)));
                }
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
        BinOp::Shl | BinOp::Shr if bc == Some(0) => {
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
