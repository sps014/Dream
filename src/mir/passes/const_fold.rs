//! Constant folding: evaluate binary/unary operations whose operands are already constants and
//! replace them with the literal result.

use super::MirPass;
use crate::mir::{BinOp, Const, MirFunction, Operand, Rvalue, Statement, UnOp};
use crate::types::TypeInterner;

pub struct ConstFold;

impl MirPass for ConstFold {
    fn name(&self) -> &'static str {
        "const-fold"
    }

    fn run(&self, func: &mut MirFunction, _interner: &TypeInterner) -> bool {
        let mut changed = false;
        for block in &mut func.blocks {
            for stmt in &mut block.stmts {
                if let Statement::Assign(_, rvalue) = stmt {
                    if let Some(folded) = fold(rvalue) {
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

fn fold(rvalue: &Rvalue) -> Option<Const> {
    match rvalue {
        Rvalue::Binary(op, a, b) => fold_binary(*op, as_const(a)?, as_const(b)?),
        Rvalue::Unary(op, a) => fold_unary(*op, as_const(a)?),
        _ => None,
    }
}

fn fold_binary(op: BinOp, a: &Const, b: &Const) -> Option<Const> {
    use Const::*;
    match (a, b) {
        (Int(x), Int(y)) => Some(fold_int(op, *x, *y)?),
        // `long`/`ulong` fold with the same i64 arithmetic but preserve the 64-bit width.
        (Long(x), Long(y)) => Some(widen_int(op, fold_int(op, *x, *y)?)),
        (Float(x), Float(y)) => Some(fold_float(op, *x, *y)?),
        (F32(x), F32(y)) => Some(narrow_float(op, fold_float(op, *x as f64, *y as f64)?)),
        (Bool(x), Bool(y)) => Some(fold_bool(op, *x, *y)?),
        _ => None,
    }
}

/// Re-widens an i64 fold result to [`Const::Long`] (comparisons already produced a `Bool`, which is
/// passed through) so `long`+`long` stays `long`.
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

fn fold_int(op: BinOp, x: i64, y: i64) -> Option<Const> {
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
        (UnOp::Neg, Const::Int(x)) => Const::Int(x.wrapping_neg()),
        (UnOp::Neg, Const::Long(x)) => Const::Long(x.wrapping_neg()),
        (UnOp::Neg, Const::Float(x)) => Const::Float(-x),
        (UnOp::Neg, Const::F32(x)) => Const::F32(-x),
        (UnOp::Not, Const::Bool(x)) => Const::Bool(!x),
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
}
