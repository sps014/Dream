//! Renumbering a cloned callee body into the caller's local/block namespaces, plus the coarse
//! argument-type inference used to decide whether a parameter binding needs a widening `Cast`.

use crate::mir::{
    BasicBlock, Const, Local, MirFunction, Operand, Place, Rvalue, Statement, Terminator,
};
use crate::types::{PrimTy, TyKind, TypeId, TypeInterner};

// --- Argument-type inference for call-boundary numeric widening. ---

/// The four WASM value types, coarsely: enough to decide whether a binding needs a widening `Cast`.
#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum WasmKind {
    I32,
    I64,
    F32,
    F64,
}

pub(super) fn wasm_kind(interner: &TypeInterner, ty: TypeId) -> WasmKind {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Double) => WasmKind::F64,
        TyKind::Prim(PrimTy::Float) => WasmKind::F32,
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => WasmKind::I64,
        _ => WasmKind::I32,
    }
}

/// The type of an argument operand, for the cases the inliner can determine statically. Returns
/// `None` for field/index/global reads (whose type needs layout resolution); the caller then either
/// binds with a plain copy (safe for `i32`-width parameters) or declines to inline (wide parameters).
pub(super) fn arg_type(caller: &MirFunction, op: &Operand, interner: &TypeInterner) -> Option<TypeId> {
    match op {
        Operand::Copy(Place::Local(l)) => Some(caller.local_ty(*l)),
        Operand::Const(c) => Some(const_type(c, interner)),
        _ => None,
    }
}

/// The type a constant operand carries (mirrors the backend's `operand_ty`): `Float` is a 64-bit
/// `double`, `F32` a 32-bit `float`; `Null` is a null pointer (`i32`).
fn const_type(c: &Const, interner: &TypeInterner) -> TypeId {
    match c {
        Const::Long(_) => interner.long(),
        Const::Float(_) => interner.double(),
        Const::F32(_) => interner.float(),
        Const::Char(_) => interner.char(),
        Const::Bool(_) => interner.bool(),
        Const::Str(_) => interner.string(),
        Const::Int(_) | Const::Null => interner.int(),
    }
}

// --- Renumbering the cloned callee body into the caller's local/block namespaces. ---

pub(super) fn remap_block(bb: &mut BasicBlock, local_base: u32, block_base: u32) {
    for s in &mut bb.stmts {
        remap_stmt(s, local_base);
    }
    remap_terminator(&mut bb.terminator, local_base, block_base);
}

fn remap_local(l: &mut Local, base: u32) {
    l.0 += base;
}

fn remap_place(p: &mut Place, base: u32) {
    match p {
        Place::Local(l) => remap_local(l, base),
        Place::Field { base: b, .. } => remap_local(b, base),
        Place::Index { base: b, index } => {
            remap_local(b, base);
            remap_operand(index, base);
        }
        Place::Global(_) => {}
    }
}

fn remap_operand(op: &mut Operand, base: u32) {
    if let Operand::Copy(p) = op {
        remap_place(p, base);
    }
}

fn remap_rvalue(rv: &mut Rvalue, base: u32) {
    match rv {
        Rvalue::Select {
            cond,
            then_val,
            else_val,
        } => {
            remap_operand(cond, base);
            remap_operand(then_val, base);
            remap_operand(else_val, base);
        }
        Rvalue::Use(o)
        | Rvalue::Unary(_, o)
        | Rvalue::ArrayLen(o)
        | Rvalue::StrLen(o)
        | Rvalue::Cast(o, _, _)
        | Rvalue::IsType(o, _)
        | Rvalue::Discriminant(o)
        | Rvalue::HashCode(o)
        | Rvalue::ToString(o)
        | Rvalue::UnionField { base: o, .. } => remap_operand(o, base),
        Rvalue::Binary(_, a, b) | Rvalue::CharAt(a, b) | Rvalue::Concat(a, b) => {
            remap_operand(a, base);
            remap_operand(b, base);
        }
        Rvalue::EnumName { value, .. } => remap_operand(value, base),
        Rvalue::ArrayNew { len, .. } => remap_operand(len, base),
        Rvalue::ToBytes { value: o, .. } | Rvalue::FromBytes { bytes: o, .. } => {
            remap_operand(o, base)
        }
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::UnionNew { args, .. }
        | Rvalue::ArrayLit { elems: args, .. } => {
            for a in args {
                remap_operand(a, base);
            }
        }
        Rvalue::IndirectCall { target, args } => {
            remap_operand(target, base);
            for a in args {
                remap_operand(a, base);
            }
        }
        Rvalue::InterfaceCall { receiver, args, .. } => {
            remap_operand(receiver, base);
            for a in args {
                remap_operand(a, base);
            }
        }
        Rvalue::JsCall {
            target,
            method,
            args,
            ..
        } => {
            remap_operand(target, base);
            if let Some(m) = method {
                remap_operand(m, base);
            }
            for (a, _) in args {
                remap_operand(a, base);
            }
        }
        Rvalue::FuncRef(_) => {}
    }
}

fn remap_stmt(s: &mut Statement, base: u32) {
    match s {
        Statement::Assign(place, rv) => {
            remap_place(place, base);
            remap_rvalue(rv, base);
        }
        Statement::Retain(o) | Statement::Release(o) => remap_operand(o, base),
        Statement::Call { args, .. } => {
            for a in args {
                remap_operand(a, base);
            }
        }
        Statement::InterfaceCall { receiver, args, .. } => {
            remap_operand(receiver, base);
            for a in args {
                remap_operand(a, base);
            }
        }
        Statement::Print { arg, .. } => remap_operand(arg, base),
        Statement::Nop | Statement::DebugLine(_) => {}
    }
}

fn remap_terminator(t: &mut Terminator, local_base: u32, block_base: u32) {
    match t {
        Terminator::Goto(b) => b.0 += block_base,
        Terminator::If {
            cond,
            then_blk,
            else_blk,
        } => {
            remap_operand(cond, local_base);
            then_blk.0 += block_base;
            else_blk.0 += block_base;
        }
        Terminator::Switch {
            value,
            targets,
            default,
        } => {
            remap_operand(value, local_base);
            for (_, b) in targets {
                b.0 += block_base;
            }
            default.0 += block_base;
        }
        Terminator::Return(Some(o)) | Terminator::AsyncComplete(Some(o)) => {
            remap_operand(o, local_base)
        }
        Terminator::Await {
            future,
            dest,
            resume,
        } => {
            remap_operand(future, local_base);
            if let Some(d) = dest {
                d.0 += local_base;
            }
            resume.0 += block_base;
        }
        Terminator::TailCall { args, .. } => {
            for a in args {
                remap_operand(a, local_base);
            }
        }
        Terminator::Return(None) | Terminator::AsyncComplete(None) | Terminator::Unreachable => {}
    }
}
