//! Async/await lowering for the MIR backend.
//!
//! An `async fun` compiles to a **constructor** (allocates a `Future` frame, stores params, enqueues
//! the first poll, returns the frame pointer) and a **poll** function (resumable state machine between
//! `await` points). The cooperative scheduler runtime lives in `mir/runtime/async.wat`.

use super::emit::{
    emit_expr_to_scratch, emit_straight_line_segment, func_symbol, poll_symbol, release_call_for_ty,
    wasm_ty_of,
};
use super::lower::{lower_async_segment, lower_expr_value};
use super::MirFunction;
use crate::hir::{HExpr, HExprKind, HStmt, LocalId};
use crate::types::{TypeId, TypeInterner};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::fmt::Write;

const F_STATE: i32 = 0;
const F_RESULT: i32 = 8;
const F_AWAITING: i32 = 20;
const F_SLOTS: i32 = 56;
const KIND_TASK: i32 = 0;
const SLOT_SIZE: i32 = 8;

const RUNTIME_ASYNC: &str = include_str!("runtime/async.wat");

pub fn poll_indices(functions: &[MirFunction]) -> HashMap<(crate::types::DefId, Vec<TypeId>), usize> {
    let base = functions.len();
    functions
        .iter()
        .filter(|f| f.is_async)
        .enumerate()
        .map(|(i, f)| ((f.def, f.instance.clone()), base + i))
        .collect()
}

pub fn module_has_async(functions: &[MirFunction]) -> bool {
    functions.iter().any(|f| f.is_async)
}

pub fn async_runtime_wat() -> String {
    const F_POLL: i32 = 12;
    const F_KIND: i32 = 24;
    const F_QUEUED: i32 = 48;
    const F_NEXT: i32 = 44;
    const F_RESULTS: i32 = 40;
    const F_RESULT: i32 = 8;
    const F_STATUS: i32 = 4;
    const F_WAKER: i32 = 16;
    const F_DUE: i32 = 52;
    const F_CHILDREN: i32 = 28;
    const F_COUNT: i32 = 32;
    const F_REMAINING: i32 = 36;
    const F_SLOTS_RT: i32 = 56;
    const KIND_ALL: i32 = 2;
    const KIND_ANY: i32 = 3;
    RUNTIME_ASYNC
        .replace("{F_POLL}", &F_POLL.to_string())
        .replace("{F_KIND}", &F_KIND.to_string())
        .replace("{F_QUEUED}", &F_QUEUED.to_string())
        .replace("{F_NEXT}", &F_NEXT.to_string())
        .replace("{F_RESULTS}", &F_RESULTS.to_string())
        .replace("{F_RESULT}", &F_RESULT.to_string())
        .replace("{F_STATUS}", &F_STATUS.to_string())
        .replace("{F_WAKER}", &F_WAKER.to_string())
        .replace("{F_DUE}", &F_DUE.to_string())
        .replace("{F_CHILDREN}", &F_CHILDREN.to_string())
        .replace("{F_COUNT}", &F_COUNT.to_string())
        .replace("{F_REMAINING}", &F_REMAINING.to_string())
        .replace("{F_SLOTS}", &F_SLOTS_RT.to_string())
        .replace("{KIND_ALL}", &KIND_ALL.to_string())
        .replace("{KIND_ANY}", &KIND_ANY.to_string())
        .replace("{tag_array}", &super::abi::TAG_ARRAY.to_string())
}

struct AsyncSlots {
    entries: Vec<(usize, String, String)>,
    offsets: HashMap<usize, i32>,
    ref_locals: Vec<usize>,
}

fn async_slots(func: &MirFunction, interner: &TypeInterner) -> AsyncSlots {
    let mut entries: Vec<(usize, String, String)> = func
        .locals
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let name = d.name.clone().unwrap_or_else(|| format!("_{i}"));
            (i, name, wasm_ty_of(interner, d.ty).to_string())
        })
        .collect();
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    let mut offsets = HashMap::new();
    let mut ref_locals = Vec::new();
    for (slot, (local_idx, _, _)) in entries.iter().enumerate() {
        offsets.insert(*local_idx, F_SLOTS + (slot as i32) * SLOT_SIZE);
        if interner.is_reference(func.locals[*local_idx].ty) {
            ref_locals.push(*local_idx);
        }
    }
    AsyncSlots {
        entries,
        offsets,
        ref_locals,
    }
}

enum AsyncResume {
    None,
    BindLocal(LocalId),
    Discard,
    ReturnAwaited,
}

enum SegmentEnd {
    Suspend(HExpr),
    CompleteVoid,
}

struct Segment {
    resume: AsyncResume,
    plain: Vec<HStmt>,
    end: SegmentEnd,
}

fn split_async_segments(body: &[HStmt]) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut resume = AsyncResume::None;
    let mut plain: Vec<HStmt> = Vec::new();

    for stmt in body {
        match stmt {
            HStmt::Let { local, value, .. } if matches!(value.kind, HExprKind::Await(_)) => {
                let HExprKind::Await(inner) = &value.kind else { unreachable!() };
                segs.push(Segment {
                    resume: std::mem::replace(&mut resume, AsyncResume::BindLocal(*local)),
                    plain: std::mem::take(&mut plain),
                    end: SegmentEnd::Suspend(*inner.clone()),
                });
            }
            HStmt::Await(e) => {
                segs.push(Segment {
                    resume: std::mem::replace(&mut resume, AsyncResume::Discard),
                    plain: std::mem::take(&mut plain),
                    end: SegmentEnd::Suspend(e.clone()),
                });
            }
            HStmt::Return(Some(e)) if matches!(e.kind, HExprKind::Await(_)) => {
                let HExprKind::Await(inner) = &e.kind else { unreachable!() };
                segs.push(Segment {
                    resume: std::mem::replace(&mut resume, AsyncResume::ReturnAwaited),
                    plain: std::mem::take(&mut plain),
                    end: SegmentEnd::Suspend(*inner.clone()),
                });
            }
            other => plain.push(other.clone()),
        }
    }
    segs.push(Segment {
        resume,
        plain,
        end: SegmentEnd::CompleteVoid,
    });
    segs
}

fn slot_store(wt: &str) -> &'static str {
    match wt {
        "f64" => "f64.store",
        "f32" => "f32.store",
        "i64" => "i64.store",
        _ => "i32.store",
    }
}

fn slot_load(wt: &str) -> &'static str {
    match wt {
        "f64" => "f64.load",
        "f32" => "f32.load",
        "i64" => "i64.load",
        _ => "i32.load",
    }
}

fn emit_release_all_locals(
    out: &mut String,
    func: &MirFunction,
    interner: &TypeInterner,
    layouts: &crate::hir::LayoutTable,
) {
    for (i, decl) in func.locals.iter().enumerate() {
        if interner.is_reference(decl.ty) {
            let call = release_call_for_ty(interner, layouts, decl.ty);
            let _ = writeln!(out, "     (local.get ${i})");
            let _ = writeln!(out, "     (call {call})");
        }
    }
}

fn emit_dream_complete(out: &mut String, value_on_stack: bool) {
    out.push_str("     (local.get $self)\n");
    if !value_on_stack {
        out.push_str("     (i32.const 0)\n");
    }
    out.push_str("     (call $dream_complete)\n     (i32.const 0)\n     (return)\n");
}

fn save_locals_to_frame(out: &mut String, slots: &AsyncSlots) {
    for (local_idx, _, wt) in &slots.entries {
        let off = slots.offsets[local_idx];
        let _ = writeln!(out, "     (local.get $self)");
        let _ = writeln!(out, "     (local.get ${local_idx})");
        let _ = writeln!(out, "     {} offset={off}", slot_store(wt));
    }
}

/// Emits the constructor + poll WAT for one async function.
#[allow(clippy::too_many_arguments)]
pub fn emit_async_function(
    func: &MirFunction,
    interner: &TypeInterner,
    symbols: &HashMap<(crate::types::DefId, Vec<TypeId>), String>,
    layouts: &crate::hir::LayoutTable,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
    ftable: &HashMap<(crate::types::DefId, Vec<TypeId>), usize>,
    poll_idx: usize,
) -> String {
    let hir = func.hir_fn.as_ref().expect("async function missing hir_fn snapshot");
    let slots = async_slots(func, interner);
    let frame_size = F_SLOTS + (slots.entries.len() as i32) * SLOT_SIZE;
    let sym = func_symbol(func);
    let segments = split_async_segments(&hir.body);
    // Segment lowering may introduce temporaries beyond the user's locals; size the poll frame to fit.
    let mut max_locals = func.locals.len();
    for seg in &segments {
        if !seg.plain.is_empty() {
            max_locals = max_locals.max(lower_async_segment(hir, &seg.plain, interner).locals.len());
        }
        if let SegmentEnd::Suspend(child) = &seg.end {
            max_locals = max_locals.max(lower_expr_value(hir, child, interner).0.locals.len());
        }
    }
    let mut out = String::new();

    let _ = writeln!(out, "(func ${sym}");
    for p in &func.params {
        let _ = writeln!(
            out,
            " (param ${} {})",
            p.0,
            wasm_ty_of(interner, func.locals[p.0 as usize].ty)
        );
    }
    out.push_str(" (result i32)\n (local $self i32)\n");
    let _ = writeln!(out, " i32.const {frame_size}");
    let _ = writeln!(out, " i32.const {poll_idx}");
    let _ = writeln!(out, " i32.const {KIND_TASK}");
    out.push_str(" call $dream_new_future\n local.set $self\n");
    for p in &func.params {
        let idx = p.0 as usize;
        let off = slots.offsets[&idx];
        let wt = wasm_ty_of(interner, func.locals[idx].ty);
        if interner.is_reference(func.locals[idx].ty) {
            let _ = writeln!(out, " local.get ${idx}");
            out.push_str(" call $retain\n");
        }
        let _ = writeln!(out, " local.get $self\n local.get ${idx}\n {} offset={off}", slot_store(wt));
    }
    out.push_str(" local.get $self\n call $dream_enqueue\n local.get $self\n)\n\n");

    let _ = writeln!(out, "(func ${} (param $self i32) (result i32)", poll_symbol(func));
    for i in 0..max_locals {
        let ty = func
            .locals
            .get(i)
            .map(|d| d.ty)
            .unwrap_or_else(|| interner.int());
        let _ = writeln!(out, " (local ${i} {})", wasm_ty_of(interner, ty));
    }
    // `$__obj`/`$__len`/`$__rel` back the same array/reassignment scratch the normal emitter uses;
    // `$__pc` drives the per-segment CFG dispatch loop (segments whose plain code has control flow).
    out.push_str(" (local $__obj i32)\n (local $__scratch i32)\n");
    out.push_str(" (local $__len i32)\n (local $__rel i32)\n (local $__pc i32)\n");
    // Saved `$__sp` across a dynamic `js` call's argument-slot buffer (see `Emitter::emit_js_call`),
    // needed here too since a `js` call can appear in an `async` body.
    out.push_str(" (local $__jsp i32)\n");

    for (local_idx, _, wt) in &slots.entries {
        let off = slots.offsets[local_idx];
        out.push_str(" local.get $self\n");
        let _ = writeln!(out, " {} offset={off}", slot_load(wt));
        let _ = writeln!(out, " local.set ${local_idx}");
        if slots.ref_locals.contains(local_idx) {
            out.push_str(" local.get $self\n i32.const 0\n");
            let _ = writeln!(out, " i32.store offset={off}");
        }
    }

    let n = segments.len();
    for k in (0..n).rev() {
        let _ = writeln!(out, " (block $async_seg{k}");
    }
    out.push_str(&format!(
        " local.get $self\n i32.load offset={F_STATE}\n br_table {}\n",
        (0..n).map(|k| format!("$async_seg{k} ")).collect::<String>()
    ));

    for (seg_idx, seg) in segments.iter().enumerate() {
        out.push_str(" )\n");
        match &seg.resume {
            AsyncResume::None => {}
            AsyncResume::BindLocal(id) => {
                let mir_local = id.0 as usize;
                out.push_str(&format!(
                    " local.get $self\n i32.load offset={F_AWAITING}\n i32.load offset={F_RESULT}\n local.set ${mir_local}\n"
                ));
            }
            AsyncResume::Discard => {}
            AsyncResume::ReturnAwaited => {
                emit_release_all_locals(&mut out, func, interner, layouts);
                out.push_str(" local.get $self\n local.get $self\n");
                let _ = writeln!(out, " i32.load offset={F_AWAITING}");
                let _ = writeln!(out, " i32.load offset={F_RESULT}");
                emit_dream_complete(&mut out, true);
                continue;
            }
        }

        if !seg.plain.is_empty() {
            let seg_mir = lower_async_segment(hir, &seg.plain, interner);
            out.push_str(&emit_straight_line_segment(
                &seg_mir, interner, symbols, layouts, strings, tags, ftable, func,
            ));
        }

        match &seg.end {
            SegmentEnd::Suspend(child) => {
                out.push_str(&emit_expr_to_scratch(
                    hir, child, interner, symbols, layouts, strings, tags, ftable, func,
                ));
                let next = seg_idx + 1;
                out.push_str(" local.get $self\n local.get $__scratch\n");
                let _ = writeln!(out, " i32.store offset={F_AWAITING}");
                out.push_str(&format!(" local.get $self\n i32.const {next}\n i32.store offset={F_STATE}\n"));
                save_locals_to_frame(&mut out, &slots);
                out.push_str(" local.get $self\n local.get $__scratch\n call $dream_await\n i32.const 0\n return\n");
            }
            SegmentEnd::CompleteVoid => {
                emit_release_all_locals(&mut out, func, interner, layouts);
                emit_dream_complete(&mut out, false);
            }
        }
    }

    out.push_str(")\n");
    out
}

pub fn emit_async_main_wrapper(entry_sym: &str, has_args_param: bool) -> String {
    let mut out = String::from("(func (export \"main\")");
    if has_args_param {
        out.push_str("\n (local $args i32)");
        out.push_str("\n i32.const 4");
        out.push_str(&format!("\n i32.const {}", super::abi::TAG_ARRAY));
        out.push_str("\n call $malloc\n local.set $args\n local.get $args\n i32.const 0\n i32.store\n local.get $args");
    }
    let _ = writeln!(out, "\n call ${entry_sym}\n drop\n call $dream_run_loop\n)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_runtime_has_no_placeholders() {
        let wat = async_runtime_wat();
        assert!(!wat.contains('{') && !wat.contains('}'));
    }
}
