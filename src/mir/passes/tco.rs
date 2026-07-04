//! Tail-call optimization. A direct call in return position — `d = f(args); return d;` or a
//! statement call immediately followed by `return;` — becomes a [`Terminator::TailCall`], which the
//! backend emits as WASM `return_call`, reusing the current frame instead of growing the stack.
//!
//! The transform is intentionally conservative for ABI safety:
//! - Only fires when the call is the *last* statement of the block and is immediately returned. If
//!   any statement (e.g. an RC `Release`) sits between the call and the return, the block is left
//!   alone — so no cleanup that must run after the call is skipped.
//! - Only all-scalar signatures qualify: neither the callee's return nor any of its parameters may
//!   be a value struct (which uses the shadow-stack / sret ABI). This guarantees the frame teardown
//!   emitted before `return_call` never frees a live argument.
//! - Async functions are skipped entirely (their poll-function control flow is special).

use super::MirPass;
use crate::mir::{MirFunction, Operand, Place, Rvalue, Statement, Terminator};
use crate::types::TypeInterner;

pub struct Tco;

impl MirPass for Tco {
    fn name(&self) -> &'static str {
        "tco"
    }

    fn run(&self, func: &mut MirFunction, interner: &TypeInterner) -> bool {
        if func.is_async {
            return false;
        }
        let ret = func.ret;
        let mut changed = false;
        for i in 0..func.blocks.len() {
            let block = &func.blocks[i];
            let Some(last) = block.stmts.last() else {
                continue;
            };
            let ok = match (&block.terminator, last) {
                // `d = f(args); return d;`
                (
                    Terminator::Return(Some(Operand::Copy(Place::Local(d)))),
                    Statement::Assign(Place::Local(d2), Rvalue::Call { args, .. }),
                ) => d == d2 && !interner.is_value_type(ret) && args_all_scalar(func, interner, args),
                // `f(args); return;`
                (Terminator::Return(None), Statement::Call { callee, args }) => {
                    callee.ret == interner.void() && args_all_scalar(func, interner, args)
                }
                _ => false,
            };
            if !ok {
                continue;
            }
            let last = func.blocks[i].stmts.pop().expect("checked non-empty");
            let (callee, args) = match last {
                Statement::Assign(_, Rvalue::Call { callee, args }) => (callee, args),
                Statement::Call { callee, args } => (callee, args),
                _ => unreachable!("only call statements are converted"),
            };
            func.blocks[i].terminator = Terminator::TailCall { callee, args };
            changed = true;
        }
        changed
    }
}

/// True if every call argument is a scalar (constant or a non-value-struct local read). This is the
/// crucial ABI guard: a value-struct argument is passed as a pointer into the *current* frame's
/// shadow stack, which the frame teardown emitted before `return_call` would free — so such calls
/// must not be tail-called. Memory-place operands are disallowed since their type isn't verifiable
/// here.
fn args_all_scalar(func: &MirFunction, interner: &TypeInterner, args: &[Operand]) -> bool {
    args.iter().all(|a| match a {
        Operand::Const(_) => true,
        Operand::Copy(Place::Local(l)) => !interner.is_value_type(func.local_ty(*l)),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Callee, DefId, Rvalue};
    use crate::types::TypeId;

    fn callee(interner: &TypeInterner, ret: TypeId) -> Callee {
        Callee {
            def: DefId(1),
            args: vec![interner.int()],
            ret,
        }
    }

    #[test]
    fn converts_value_tail_call() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let d = b.new_temp(i.int());
        b.assign(
            Place::Local(d),
            Rvalue::Call {
                callee: callee(&i, i.int()),
                args: vec![Operand::Const(crate::mir::Const::Int(1))],
            },
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(d)))));
        let mut func = b.finish();
        assert!(Tco.run(&mut func, &i));
        assert!(matches!(func.blocks[0].terminator, Terminator::TailCall { .. }));
        assert!(func.blocks[0].stmts.is_empty(), "call statement folded away");
    }

    #[test]
    fn skips_when_release_after_call() {
        // `d = f(); release(d); return d;` — cleanup between call and return blocks the tail call.
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("f", i.int());
        let d = b.new_temp(i.int());
        b.assign(
            Place::Local(d),
            Rvalue::Call {
                callee: callee(&i, i.int()),
                args: vec![],
            },
        );
        b.push(Statement::Release(Operand::Copy(Place::Local(d))));
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(d)))));
        let mut func = b.finish();
        assert!(!Tco.run(&mut func, &i), "must not tail-call past cleanup");
    }
}
