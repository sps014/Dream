//! A compact textual dump of MIR, for tests and `--emit=mir`-style debugging. Renders blocks in
//! order with their statements and terminator.

use super::{
    BasicBlock, Const, MirFunction, Operand, Place, Rvalue, Statement, Terminator,
};
use std::fmt::Write;

pub fn print_function(func: &MirFunction) -> String {
    let mut out = String::new();
    let params: Vec<String> = func.params.iter().map(|l| format!("_{}", l.0)).collect();
    let _ = writeln!(out, "fn {}({}) {{", func.name, params.join(", "));
    for (i, block) in func.blocks.iter().enumerate() {
        let _ = writeln!(out, "  bb{}:", i);
        print_block(&mut out, block);
    }
    let _ = writeln!(out, "}}");
    out
}

fn print_block(out: &mut String, block: &BasicBlock) {
    for s in &block.stmts {
        let _ = writeln!(out, "    {}", stmt(s));
    }
    let _ = writeln!(out, "    {}", terminator(&block.terminator));
}

fn stmt(s: &Statement) -> String {
    match s {
        Statement::Assign(p, r) => format!("{} = {}", place(p), rvalue(r)),
        Statement::Retain(o) => format!("retain {}", operand(o)),
        Statement::Release(o) => format!("release {}", operand(o)),
        Statement::Call { callee, args } => {
            format!("call def{}({})", callee.def.0, ops(args))
        }
        Statement::InterfaceCall { receiver, iface_id, method_slot, args, .. } => {
            format!(
                "iface_call I{}#{} {}({})",
                iface_id,
                method_slot,
                operand(receiver),
                ops(args)
            )
        }
        Statement::Print { arg, newline, .. } => {
            let f = if *newline { "println" } else { "print" };
            format!("{}({})", f, operand(arg))
        }
        Statement::Nop => "nop".to_string(),
    }
}

fn terminator(t: &Terminator) -> String {
    match t {
        Terminator::Goto(b) => format!("goto bb{}", b.0),
        Terminator::If { cond, then_blk, else_blk } => {
            format!("if {} -> bb{} else bb{}", operand(cond), then_blk.0, else_blk.0)
        }
        Terminator::Switch { value, targets, default } => {
            let arms: Vec<String> = targets
                .iter()
                .map(|(v, b)| format!("{} -> bb{}", v, b.0))
                .collect();
            format!("switch {} [{}] else bb{}", operand(value), arms.join(", "), default.0)
        }
        Terminator::Return(Some(o)) => format!("return {}", operand(o)),
        Terminator::Return(None) => "return".to_string(),
        Terminator::AsyncComplete(v) => format!(
            "async_complete{}",
            v.as_ref().map(operand).unwrap_or_default()
        ),
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

fn rvalue(r: &Rvalue) -> String {
    match r {
        Rvalue::Use(o) => operand(o),
        Rvalue::Binary(op, a, b) => format!("{:?}({}, {})", op, operand(a), operand(b)),
        Rvalue::Unary(op, a) => format!("{:?}({})", op, operand(a)),
        Rvalue::Call { callee, args } => format!("call def{}({})", callee.def.0, ops(args)),
        Rvalue::IndirectCall { target, args } => {
            format!("call_indirect {}({})", operand(target), ops(args))
        }
        Rvalue::InterfaceCall { receiver, iface_id, method_slot, args, .. } => {
            format!(
                "iface_call I{}#{} {}({})",
                iface_id,
                method_slot,
                operand(receiver),
                ops(args)
            )
        }
        Rvalue::New { def, args, .. } => format!("new def{}({})", def.0, ops(args)),
        Rvalue::UnionNew { def, variant, args, .. } => {
            format!("union def{}#{}({})", def.0, variant, ops(args))
        }
        Rvalue::ArrayLit { elems, .. } => format!("[{}]", ops(elems)),
        Rvalue::ArrayLen(o) => format!("len({})", operand(o)),
        Rvalue::StrLen(o) => format!("strlen({})", operand(o)),
        Rvalue::CharAt(s, i) => format!("char_at({}, {})", operand(s), operand(i)),
        Rvalue::ArrayNew { elem_ty, len } => {
            format!("array_new::<ty{}>({})", elem_ty.0, operand(len))
        }
        Rvalue::HashCode(o) => format!("hash_code({})", operand(o)),
        Rvalue::ToString(o) => format!("to_string({})", operand(o)),
        Rvalue::Concat(a, b) => format!("concat({}, {})", operand(a), operand(b)),
        Rvalue::EnumName { value, .. } => format!("enum_name({})", operand(value)),
        Rvalue::Cast(o, from, ty) => format!("{} as ty{} (from ty{})", operand(o), ty.0, from.0),
        Rvalue::Discriminant(o) => format!("discriminant({})", operand(o)),
        Rvalue::IsType(o, ty) => format!("{} is ty{}", operand(o), ty.0),
        Rvalue::UnionField { base, variant, field, .. } => {
            format!("{}#{}.{}", operand(base), variant, field)
        }
        Rvalue::FuncRef(callee) => format!("funcref def{}", callee.def.0),
        Rvalue::JsCall { callee, target, method, args } => {
            let m = method.as_ref().map(|m| operand(m)).unwrap_or_else(|| "*".to_string());
            let a = args.iter().map(|(o, _)| operand(o)).collect::<Vec<_>>().join(", ");
            format!("js_call def{} {}[{}]({})", callee.def.0, operand(target), m, a)
        }
    }
}

fn operand(o: &Operand) -> String {
    match o {
        Operand::Copy(p) => place(p),
        Operand::Const(c) => constant(c),
    }
}

fn ops(list: &[Operand]) -> String {
    list.iter().map(operand).collect::<Vec<_>>().join(", ")
}

fn place(p: &Place) -> String {
    match p {
        Place::Local(l) => format!("_{}", l.0),
        Place::Global(g) => format!("@{}", g.0),
        Place::Field { base, field } => format!("_{}.{}", base.0, field),
        Place::Index { base, index } => format!("_{}[{}]", base.0, operand(index)),
    }
}

fn constant(c: &Const) -> String {
    match c {
        Const::Int(v) => v.to_string(),
        Const::Long(v) => format!("{}L", v),
        Const::Float(v) => v.to_string(),
        Const::F32(v) => format!("{}f", v),
        Const::Bool(v) => v.to_string(),
        Const::Char(v) => format!("'{}'", v),
        Const::Str(s) => format!("{:?}", s),
        Const::Null => "null".to_string(),
    }
}
