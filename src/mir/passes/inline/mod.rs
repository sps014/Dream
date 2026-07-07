//! Function inlining.
//!
//! Replaces a direct call with a copy of the callee's body, wired into the caller's CFG: the caller
//! block is split at the call, the callee's locals/blocks are renumbered into the caller, parameters
//! are bound to the argument operands, and every callee `Return` becomes a jump to a continuation
//! block (assigning the returned value into the call's destination first).
//!
//! Inlining runs as a [`ModulePass`] *before* per-function `RcInsertion`, so the merged body gets
//! reference-counting and the local optimization pipeline uniformly — no cross-function RC reasoning
//! is needed. Only direct `Call`s to synchronous, non-recursive functions are inlined; constructors
//! (`New`), indirect/interface calls, and address-taken-only edges are left alone.

use super::ModulePass;
use crate::mir::{
    BasicBlock, BlockId, Const, Local, Operand, Place, Rvalue, Statement, Terminator,
};
use crate::types::{DefId, TypeId, TypeInterner};
use std::collections::{HashMap, HashSet};

mod graph;
mod remap;

use graph::{address_taken, count_call_sites, recursive_set};
use remap::{arg_type, remap_block, wasm_kind, WasmKind};

/// A function's call-graph identity (matches `MirFunction::{def, instance}` and `Callee::{def,args}`).
type FnKey = (DefId, Vec<TypeId>);

/// A callee small enough to always inline: at most this many statements across all its blocks.
const MAX_INLINE_STMTS: usize = 24;
/// ...and at most this many blocks.
const MAX_INLINE_BLOCKS: usize = 6;
/// Stop inlining into a caller once it has grown past this many blocks, to bound code blow-up.
const CALLER_BLOCK_CAP: usize = 4096;
/// Safety cap on inlines performed into a single function per `run` (defends against any unforeseen
/// non-termination; the DAG-only inlining should terminate well before this).
const MAX_INLINES_PER_FN: usize = 4096;

#[derive(Default)]
pub struct Inliner;

impl ModulePass for Inliner {
    fn name(&self) -> &'static str {
        "inline"
    }

    fn run(&self, mir: &mut crate::mir::Mir, interner: &TypeInterner) -> bool {
        let index: HashMap<FnKey, usize> = mir
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| ((f.def, f.instance.clone()), i))
            .collect();
        let call_counts = count_call_sites(mir);
        let addr_taken = address_taken(mir);
        let recursive = recursive_set(mir, &index);

        let mut changed = false;
        for fi in 0..mir.functions.len() {
            let mut inlined = 0;
            while inlined < MAX_INLINES_PER_FN {
                if mir.functions[fi].blocks.len() > CALLER_BLOCK_CAP {
                    break;
                }
                let Some(site) = find_site(
                    mir,
                    fi,
                    &index,
                    &recursive,
                    &call_counts,
                    &addr_taken,
                    interner,
                ) else {
                    break;
                };
                perform_inline(mir, fi, site, interner);
                changed = true;
                inlined += 1;
            }
        }
        changed
    }
}

/// A resolved, inlinable call site within the caller.
struct Site {
    /// Index of the caller block containing the call.
    block: usize,
    /// Index of the call statement within that block.
    stmt: usize,
    /// Index (in `mir.functions`) of the callee to inline.
    callee: usize,
    /// The call's argument operands (caller-side, no remapping needed).
    args: Vec<Operand>,
    /// Where the returned value is written (`None` for effect-only calls).
    dest: Option<Place>,
}

/// Finds the first inlinable call site in function `fi`, or `None`.
#[allow(clippy::too_many_arguments)]
fn find_site(
    mir: &crate::mir::Mir,
    fi: usize,
    index: &HashMap<FnKey, usize>,
    recursive: &HashSet<FnKey>,
    call_counts: &HashMap<FnKey, usize>,
    addr_taken: &HashSet<FnKey>,
    interner: &TypeInterner,
) -> Option<Site> {
    let f = &mir.functions[fi];
    for (bi, block) in f.blocks.iter().enumerate() {
        for (si, stmt) in block.stmts.iter().enumerate() {
            let (key, args, dest) = match stmt {
                Statement::Call { callee, args } => {
                    ((callee.def, callee.args.clone()), args.clone(), None)
                }
                Statement::Assign(place, Rvalue::Call { callee, args }) => (
                    (callee.def, callee.args.clone()),
                    args.clone(),
                    Some(place.clone()),
                ),
                _ => continue,
            };
            let Some(&ci) = index.get(&key) else { continue };
            if !eligible(
                mir,
                fi,
                ci,
                &key,
                recursive,
                call_counts,
                addr_taken,
                &args,
                interner,
            ) {
                continue;
            }
            return Some(Site {
                block: bi,
                stmt: si,
                callee: ci,
                args,
                dest,
            });
        }
    }
    None
}

/// Whether callee `ci` may be inlined into caller `fi` at a site passing `n_args`.
#[allow(clippy::too_many_arguments)]
fn eligible(
    mir: &crate::mir::Mir,
    fi: usize,
    ci: usize,
    key: &FnKey,
    recursive: &HashSet<FnKey>,
    call_counts: &HashMap<FnKey, usize>,
    addr_taken: &HashSet<FnKey>,
    args: &[Operand],
    interner: &TypeInterner,
) -> bool {
    if ci == fi {
        return false; // direct self-recursion
    }
    let g = &mir.functions[ci];
    if g.is_async {
        return false; // async bodies are stubs; real control flow lives in the HIR snapshot
    }
    if recursive.contains(key) {
        return false; // part of a call cycle: inlining could not terminate
    }
    if g.name == crate::mir::abi::ENTRY_FN || g.name == crate::mir::lower::INIT_FN_NAME {
        return false;
    }
    if g.params.len() != args.len() || g.blocks.is_empty() {
        return false;
    }
    // Value-struct locals are torn down by the *emitter* at the function's frame teardown (shadow
    // stack), not via MIR `Release` statements. Inlining such a callee would relocate that teardown to
    // the caller's scope exit, changing when destructors run (deterministic destruction is
    // observable). Skip callees that own any value-type local or parameter.
    if g.locals.iter().any(|d| interner.is_value_type(d.ty)) {
        return false;
    }
    // A call widens each argument to the callee's parameter WASM type at the boundary (e.g. `int` ->
    // `double`). Inlining replaces that with a binding, which must carry the same widening. We can only
    // emit the widening `Cast` when the argument's type is known. If a parameter's WASM type is wider
    // than `i32` and the argument's type is indeterminate (a field/index/global read), skip inlining
    // rather than risk an i32/i64/f32/f64 mismatch in the merged body.
    let caller = &mir.functions[fi];
    for (i, param) in g.params.iter().enumerate() {
        let pty = g.local_ty(*param);
        if wasm_kind(interner, pty) != WasmKind::I32
            && arg_type(caller, &args[i], interner).is_none()
        {
            return false;
        }
    }
    let stmt_count: usize = g.blocks.iter().map(|b| b.stmts.len()).sum();
    let small = stmt_count <= MAX_INLINE_STMTS && g.blocks.len() <= MAX_INLINE_BLOCKS;
    // Always inline a function with a single direct call site whose address is never taken (it will
    // become dead and be pruned), even if it is larger than the "small" threshold.
    let single_use = call_counts.get(key).copied().unwrap_or(0) == 1 && !addr_taken.contains(key);
    small || single_use
}

/// Performs the inline described by `site` into function `fi`.
fn perform_inline(mir: &mut crate::mir::Mir, fi: usize, site: Site, interner: &TypeInterner) {
    // Clone the callee's shape before mutating the caller (they share `mir.functions`).
    let (g_entry, g_params, g_locals, g_blocks) = {
        let g = &mir.functions[site.callee];
        (
            g.entry,
            g.params.clone(),
            g.locals.clone(),
            g.blocks.clone(),
        )
    };

    let f = &mut mir.functions[fi];
    let local_base = f.locals.len() as u32;
    for decl in &g_locals {
        f.locals.push(decl.clone());
    }
    let block_base = f.blocks.len() as u32;
    let cont_id = BlockId(block_base + g_blocks.len() as u32);

    // Split the caller block at the call: statements before it stay; those after move to `cont`.
    let orig_term = f.blocks[site.block].terminator.clone();
    let tail: Vec<Statement> = f.blocks[site.block].stmts.split_off(site.stmt + 1);
    f.blocks[site.block].stmts.pop(); // remove the call statement itself
                                      // Bind parameters to the argument operands, applying the same numeric widening the call ABI would
                                      // (a narrower argument passed to a wider parameter), then jump into the (renumbered) callee entry.
    let params: std::collections::HashSet<u32> = g_params.iter().map(|p| p.0).collect();
    for (i, p) in g_params.iter().enumerate() {
        let dest_local = Local(local_base + p.0);
        let pty = g_locals[p.0 as usize].ty;
        let arg = site.args[i].clone();
        let rvalue = match arg_type(&mir.functions[fi], &arg, interner) {
            Some(aty) if wasm_kind(interner, aty) != wasm_kind(interner, pty) => {
                Rvalue::Cast(arg, aty, pty)
            }
            _ => Rvalue::Use(arg),
        };
        // Re-borrow the caller after the immutable `arg_type` read above.
        mir.functions[fi].blocks[site.block]
            .stmts
            .push(Statement::Assign(Place::Local(dest_local), rvalue));
    }
    let f = &mut mir.functions[fi];
    // Zero-initialize the callee's non-parameter *reference* locals. In a standalone function these
    // start null (a fresh WASM frame); the callee's reference-counting relies on that — its
    // release-before-overwrite and scope-exit `Release`s assume a null baseline. Inlined into the
    // caller's frame the locals persist across executions (e.g. loop iterations), so without this
    // reset a scope-exit release on a not-yet-assigned path would free a stale pointer left by a
    // previous execution (double-free / use-after-free). Emitting the reset in the site block runs it
    // once per entry into the inlined region, matching the callee's once-at-entry zeroing.
    for (i, decl) in g_locals.iter().enumerate() {
        if !params.contains(&(i as u32)) && interner.is_reference(decl.ty) {
            f.blocks[site.block].stmts.push(Statement::Assign(
                Place::Local(Local(local_base + i as u32)),
                Rvalue::Use(Operand::Const(Const::Null)),
            ));
        }
    }
    f.blocks[site.block].terminator = Terminator::Goto(BlockId(block_base + g_entry.0));

    // Append the renumbered callee blocks, turning `Return`s into jumps to `cont`.
    for mut bb in g_blocks {
        remap_block(&mut bb, local_base, block_base);
        match std::mem::replace(&mut bb.terminator, Terminator::Goto(cont_id)) {
            Terminator::Return(op) | Terminator::AsyncComplete(op) => {
                if let (Some(dest), Some(o)) = (&site.dest, op) {
                    bb.stmts
                        .push(Statement::Assign(dest.clone(), Rvalue::Use(o)));
                }
            }
            other => bb.terminator = other,
        }
        f.blocks.push(bb);
    }
    // The continuation carries the caller's original post-call statements + terminator.
    f.blocks.push(BasicBlock {
        stmts: tail,
        terminator: orig_term,
    });
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Const, MirFunction};
    use crate::types::{DefKind, TypeCtx};

    /// Builds `fun callee(a: int): int { return a + 1; }` and `fun caller(): int { return callee(41); }`
    /// and checks the call is replaced by the inlined body (no residual `Call`).
    #[test]
    fn inlines_small_callee() {
        let mut ctx = TypeCtx::new();
        let int = ctx.interner.int();
        let callee_def = ctx.register(DefKind::Function, "callee", vec![]);
        let caller_def = ctx.register(DefKind::Function, "caller", vec![]);

        let callee = {
            let mut b = FunctionBuilder::new("callee", int);
            b.set_def(callee_def, vec![]);
            let a = b.new_param(int, Some("a".into()));
            let t = b.new_temp(int);
            b.assign(
                Place::Local(t),
                Rvalue::Binary(
                    crate::mir::BinOp::Add,
                    Operand::Copy(Place::Local(a)),
                    Operand::Const(Const::Int(1)),
                ),
            );
            b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));
            b.finish()
        };
        let caller = {
            let mut b = FunctionBuilder::new("caller", int);
            b.set_def(caller_def, vec![]);
            let r = b.new_temp(int);
            b.assign(
                Place::Local(r),
                Rvalue::Call {
                    callee: crate::mir::Callee {
                        def: callee_def,
                        args: vec![],
                        ret: int,
                    },
                    args: vec![Operand::Const(Const::Int(41))],
                },
            );
            b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(r)))));
            b.finish()
        };

        let mut mir = crate::mir::Mir {
            functions: vec![callee, caller],
            ..Default::default()
        };
        assert!(Inliner.run(&mut mir, &ctx.interner));
        let caller: &MirFunction = mir.functions.iter().find(|f| f.name == "caller").unwrap();
        let has_call = caller.blocks.iter().flat_map(|b| &b.stmts).any(|s| {
            matches!(
                s,
                Statement::Call { .. } | Statement::Assign(_, Rvalue::Call { .. })
            )
        });
        assert!(!has_call, "call to callee should have been inlined away");
    }

    /// A directly self-recursive function must not be inlined into itself.
    #[test]
    fn skips_recursion() {
        let mut ctx = TypeCtx::new();
        let int = ctx.interner.int();
        let def = ctx.register(DefKind::Function, "rec", vec![]);
        let mut b = FunctionBuilder::new("rec", int);
        b.set_def(def, vec![]);
        let t = b.new_temp(int);
        b.assign(
            Place::Local(t),
            Rvalue::Call {
                callee: crate::mir::Callee {
                    def,
                    args: vec![],
                    ret: int,
                },
                args: vec![],
            },
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));
        let mut mir = crate::mir::Mir {
            functions: vec![b.finish()],
            ..Default::default()
        };
        assert!(!Inliner.run(&mut mir, &ctx.interner));
    }
}
