//! Constant folding: evaluate binary/unary operations whose operands are already constants and
//! replace them with the literal result.

use super::MirPass;
use crate::mir::{BinOp, Const, MirFunction, Operand, Place, Rvalue, Statement, UnOp};
use crate::types::{PrimTy, TyKind, TypeInterner};

pub struct ConstFold;

impl MirPass for ConstFold {
    fn name(&self) -> &'static str {
        "const-fold"
    }

    fn run(&self, func: &mut MirFunction, interner: &TypeInterner) -> bool {
        let mut changed = false;
        let locals = &func.locals;
        for block in &mut func.blocks {
            for stmt in &mut block.stmts {
                if let Statement::Assign(Place::Local(local), rvalue) = stmt {
                    let is_byte = matches!(
                        interner.kind(locals[local.0 as usize].ty),
                        TyKind::Prim(PrimTy::Byte)
                    );
                    if let Some(folded) = fold(rvalue, is_byte) {
                        *rvalue = Rvalue::Use(Operand::Const(folded));
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

fn as_const(op: &Operand) -> Option<&Const> {
    match op {
        Operand::Const(c) => Some(c),
        _ => None,
    }
}

/// `is_byte` reflects the *destination*'s declared type: `byte + byte` stays `byte` (see
/// `docs/language/primitives.md#integer-overflow`), so a fold feeding a `byte`-typed place must
/// mask into `[0, 255]` the same way the emitter's runtime `byte` ops do — folding away the
/// `Binary`/`Unary` node would otherwise silently skip that masking permanently (no later pass
/// re-derives it, since the destination's own arithmetic is gone).
pub(super) fn fold(rvalue: &Rvalue, is_byte: bool) -> Option<Const> {
    let folded = match rvalue {
        Rvalue::Binary(op, a, b) => fold_binary(*op, as_const(a)?, as_const(b)?)?,
        Rvalue::Unary(op, a) => fold_unary(*op, as_const(a)?)?,
        _ => return None,
    };
    Some(match folded {
        Const::Int(v) if is_byte => Const::Int(v & 0xFF),
        other => other,
    })
}

fn fold_binary(op: BinOp, a: &Const, b: &Const) -> Option<Const> {
    use Const::*;
    match (a, b) {
        (Int(x), Int(y)) => Some(fold_int(op, *x, *y)?),
        // `long`/`ulong` are natively 64-bit, so their arithmetic must wrap at 64 bits (plain
        // `i64` wrapping ops), never truncated through `i32` the way `int`/`uint`/`byte` are.
        (Long(x), Long(y)) => Some(widen_int(op, fold_long(op, *x, *y)?)),
        (Float(x), Float(y)) => Some(fold_float(op, *x, *y)?),
        (F32(x), F32(y)) => Some(narrow_float(op, fold_float(op, *x as f64, *y as f64)?)),
        (Bool(x), Bool(y)) => Some(fold_bool(op, *x, *y)?),
        _ => None,
    }
}

/// Re-tags a [`fold_long`] result as [`Const::Long`] (comparisons already produced a `Bool`, which
/// is passed through) so `long`+`long` stays `long`.
fn widen_int(_op: BinOp, folded: Const) -> Const {
    match folded {
        Const::Int(v) => Const::Long(v),
        other => other,
    }
}

/// Re-narrows an f64 fold result to [`Const::F32`] (comparisons pass through as `Bool`) so
/// `float`+`float` stays `float`.
fn narrow_float(_op: BinOp, folded: Const) -> Const {
    match folded {
        Const::Float(v) => Const::F32(v as f32),
        other => other,
    }
}

/// `Const::Int` represents any `i32`-register value (`int`/`uint`/`byte`) as an `i64` host integer
/// wide enough to hold `uint`'s full face-value range (`0..=4294967295`), which does *not* fit in
/// a signed `i32`. An arithmetic result that overflows 32 bits must wrap back into that same
/// 32-bit register at the same width the runtime `i32.add`/`i32.sub`/`i32.mul`/`i32.shl`
/// instructions wrap at — not at `i64` width, which is wide enough that e.g. `uint.max + 1` would
/// otherwise fold to `4294967296`, a value that fits no 32-bit register and that the WAT assembler
/// rejects outright as `i32.const 4294967296`. Truncating through `i32` yields the correct wrapped
/// *bit pattern*; re-widening to `i64` keeps it in signed form, which is always a valid `i32.const`
/// spelling of that same bit pattern (WASM doesn't distinguish signed/unsigned constants — only the
/// consuming instruction, e.g. `i32.lt_s` vs `i32.lt_u`, does), matching this file's overflow
/// policy: every integer primitive wraps (see `docs/language/primitives.md#integer-overflow`).
fn wrap32(x: i64) -> i64 {
    (x as i32) as i64
}

fn fold_int(op: BinOp, x: i64, y: i64) -> Option<Const> {
    Some(match op {
        BinOp::Add => Const::Int(wrap32(x.wrapping_add(y))),
        BinOp::Sub => Const::Int(wrap32(x.wrapping_sub(y))),
        BinOp::Mul => Const::Int(wrap32(x.wrapping_mul(y))),
        BinOp::Div if y != 0 => Const::Int(x.wrapping_div(y)),
        BinOp::Rem if y != 0 => Const::Int(x.wrapping_rem(y)),
        BinOp::BitAnd => Const::Int(x & y),
        BinOp::BitOr => Const::Int(x | y),
        BinOp::BitXor => Const::Int(x ^ y),
        BinOp::Shl => Const::Int(wrap32(x.wrapping_shl(y as u32))),
        BinOp::Shr => Const::Int(x.wrapping_shr(y as u32)),
        BinOp::Eq => Const::Bool(x == y),
        BinOp::Ne => Const::Bool(x != y),
        BinOp::Lt => Const::Bool(x < y),
        BinOp::Le => Const::Bool(x <= y),
        BinOp::Gt => Const::Bool(x > y),
        BinOp::Ge => Const::Bool(x >= y),
        // Division/modulo by zero is left for runtime to trap.
        _ => return None,
    })
}

/// 64-bit counterpart of [`fold_int`]: `long`/`ulong` arithmetic wraps at the full `i64` width,
/// so no `wrap32` truncation applies here.
fn fold_long(op: BinOp, x: i64, y: i64) -> Option<Const> {
    Some(match op {
        BinOp::Add => Const::Int(x.wrapping_add(y)),
        BinOp::Sub => Const::Int(x.wrapping_sub(y)),
        BinOp::Mul => Const::Int(x.wrapping_mul(y)),
        BinOp::Div if y != 0 => Const::Int(x.wrapping_div(y)),
        BinOp::Rem if y != 0 => Const::Int(x.wrapping_rem(y)),
        BinOp::BitAnd => Const::Int(x & y),
        BinOp::BitOr => Const::Int(x | y),
        BinOp::BitXor => Const::Int(x ^ y),
        BinOp::Shl => Const::Int(x.wrapping_shl(y as u32)),
        BinOp::Shr => Const::Int(x.wrapping_shr(y as u32)),
        BinOp::Eq => Const::Bool(x == y),
        BinOp::Ne => Const::Bool(x != y),
        BinOp::Lt => Const::Bool(x < y),
        BinOp::Le => Const::Bool(x <= y),
        BinOp::Gt => Const::Bool(x > y),
        BinOp::Ge => Const::Bool(x >= y),
        // Division/modulo by zero is left for runtime to trap.
        _ => return None,
    })
}

fn fold_float(op: BinOp, x: f64, y: f64) -> Option<Const> {
    Some(match op {
        BinOp::Add => Const::Float(x + y),
        BinOp::Sub => Const::Float(x - y),
        BinOp::Mul => Const::Float(x * y),
        BinOp::Div => Const::Float(x / y),
        BinOp::Eq => Const::Bool(x == y),
        BinOp::Ne => Const::Bool(x != y),
        BinOp::Lt => Const::Bool(x < y),
        BinOp::Le => Const::Bool(x <= y),
        BinOp::Gt => Const::Bool(x > y),
        BinOp::Ge => Const::Bool(x >= y),
        _ => return None,
    })
}

fn fold_bool(op: BinOp, x: bool, y: bool) -> Option<Const> {
    Some(match op {
        BinOp::And => Const::Bool(x && y),
        BinOp::Or => Const::Bool(x || y),
        BinOp::Eq => Const::Bool(x == y),
        BinOp::Ne => Const::Bool(x != y),
        _ => return None,
    })
}

fn fold_unary(op: UnOp, a: &Const) -> Option<Const> {
    Some(match (op, a) {
        (UnOp::Neg, Const::Int(x)) => Const::Int(wrap32(x.wrapping_neg())),
        (UnOp::Neg, Const::Long(x)) => Const::Long(x.wrapping_neg()),
        (UnOp::Neg, Const::Float(x)) => Const::Float(-x),
        (UnOp::Neg, Const::F32(x)) => Const::F32(-x),
        (UnOp::Not, Const::Bool(x)) => Const::Bool(!x),
        // `wrap32` truncates the flipped bit pattern back to the value's native 32-bit register
        // width (see its doc comment) — the same width `x ^ -1` flips at in codegen. `byte`'s
        // further `[0, 255]` mask is applied uniformly by `fold`'s caller for any `is_byte`
        // destination, exactly as for the binary integer folds above.
        (UnOp::BitNot, Const::Int(x)) => Const::Int(wrap32(!x)),
        (UnOp::BitNot, Const::Long(x)) => Const::Long(!x),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Operand, Place, Rvalue, Terminator};
    use crate::types::TypeInterner;

    #[test]
    fn folds_int_add() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let t = b.new_temp(i.int());
        b.assign(
            Place::Local(t),
            Rvalue::Binary(
                BinOp::Add,
                Operand::Const(Const::Int(2)),
                Operand::Const(Const::Int(3)),
            ),
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));
        let mut func = b.finish();
        assert!(ConstFold.run(&mut func, &i));
        match &func.blocks[0].stmts[0] {
            Statement::Assign(_, Rvalue::Use(Operand::Const(Const::Int(v)))) => assert_eq!(*v, 5),
            other => panic!("expected folded const, got {:?}", other),
        }
    }

    #[test]
    fn folds_int_add_wraps_at_32_bits() {
        // uint.max + 1 must wrap to 0, not fold to the i64 value 4294967296 (which the WAT
        // assembler would reject as an out-of-range i32.const).
        assert_eq!(fold_int(BinOp::Add, 4294967295, 1), Some(Const::Int(0)));
    }

    #[test]
    fn folds_long_add_does_not_truncate_to_32_bits() {
        // long.max + 1 must wrap at 64 bits, not get truncated through i32 the way int/uint/byte
        // do (a regression this test would have caught: sharing fold_int between int and long).
        assert_eq!(
            fold_long(BinOp::Add, i64::MAX, 1),
            Some(Const::Int(i64::MIN))
        );
    }

    #[test]
    fn folds_byte_add_masks_into_0_255() {
        // byte + byte stays byte (no C-style promotion to int), so a fold across the byte
        // arithmetic must mask the result the same way the emitter's runtime byte ops do.
        let rv = Rvalue::Binary(
            BinOp::Add,
            Operand::Const(Const::Int(250)),
            Operand::Const(Const::Int(10)),
        );
        assert_eq!(fold(&rv, true), Some(Const::Int(4)));
        assert_eq!(fold(&rv, false), Some(Const::Int(260)));
    }
}
