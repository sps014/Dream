//! Async/await lowering for the MIR backend.
//!
//! An `async fun` compiles to a **constructor** (allocates a `Future` frame, stores params, enqueues
//! the first poll, returns the frame pointer) and a **poll** function (resumable state machine between
//! `await` points). The cooperative scheduler runtime lives in `mir/runtime/async.wat`.

use super::emit::{emit_async_poll, func_symbol, poll_symbol, wasm_ty_of};
use super::lower::lower_async_poll_body;
use super::MirFunction;
use crate::types::{TypeId, TypeInterner};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::fmt::Write;

pub(crate) const F_STATE: i32 = 0;
pub(crate) const F_RESULT: i32 = 8;
pub(crate) const F_AWAITING: i32 = 20;
/// Byte size of a `Future` frame's fixed header region (locals are appended past it). Shared with
/// the emitter's `sleep` intrinsic and the host bridge (`execution/host/http.rs`,
/// `runtime/dream.js`), which allocate host futures of exactly this size.
pub(crate) const F_SLOTS: i32 = 56;
const KIND_TASK: i32 = 0;
/// `Future.kind` for a host-driven future (timer / HTTP / extern async): settled by the host via
/// `__dream_resolve` rather than by re-polling Dream code.
pub(crate) const KIND_HOST: i32 = 1;
/// Poll-table index stored in a host future; `-1` means "no Dream poll function" (the host settles
/// it directly), so the scheduler never dispatches through the function table for it.
pub(crate) const HOST_POLL_INDEX: i32 = -1;
const SLOT_SIZE: i32 = 8;

const RUNTIME_ASYNC: &str = include_str!("runtime/async.wat");

pub fn poll_indices(
    functions: &[MirFunction],
) -> HashMap<(crate::types::DefId, Vec<TypeId>), usize> {
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
    const KIND_ALL: i32 = 2;
    const KIND_ANY: i32 = 3;
    const STATUS_CANCELLED: i32 = 2;
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
        .replace("{F_SLOTS}", &F_SLOTS.to_string())
        .replace("{KIND_ALL}", &KIND_ALL.to_string())
        .replace("{KIND_ANY}", &KIND_ANY.to_string())
        .replace("{STATUS_CANCELLED}", &STATUS_CANCELLED.to_string())
        .replace("{tag_array}", &super::abi::TAG_ARRAY.to_string())
}

pub(crate) struct AsyncSlots {
    /// `(local index, name, wasm type)` for every frame-resident local, in save/load order.
    pub(crate) entries: Vec<(usize, String, String)>,
    /// Local index → byte offset of its slot within the `Future` frame.
    pub(crate) offsets: HashMap<usize, i32>,
    /// Indices of reference-typed locals (retained across a suspend, released on completion).
    pub(crate) ref_locals: Vec<usize>,
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

pub(crate) fn slot_store(wt: &str) -> &'static str {
    match wt {
        "f64" => "f64.store",
        "f32" => "f32.store",
        "i64" => "i64.store",
        _ => "i32.store",
    }
}

pub(crate) fn slot_load(wt: &str) -> &'static str {
    match wt {
        "f64" => "f64.load",
        "f32" => "f32.load",
        "i64" => "i64.load",
        _ => "i32.load",
    }
}

/// Emits the constructor + poll WAT for one async function. The whole body is lowered to a coroutine
/// CFG ([`lower_async_poll_body`]) in which `await` is a [`super::Terminator::Await`] suspend point;
/// the poll is then a single state-machine dispatch (see [`emit_async_poll`]) that resumes at the
/// `resume` block recorded in the future's `state`, so awaits work in any control-flow position.
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
    debug: bool,
    debug_fn: Option<&crate::mir::emit::debug_map::DebugFunction>,
) -> String {
    let hir = func.hir_fn.as_ref().unwrap_or_else(|| {
        crate::internal_error!(
            "async function '{}' reached codegen without its HIR snapshot",
            func.name
        )
    });
    // The coroutine body carries all frame-resident locals (user locals + await/scratch temps).
    let body = lower_async_poll_body(hir, interner);
    let slots = async_slots(&body, interner);
    let frame_size = F_SLOTS + (slots.entries.len() as i32) * SLOT_SIZE;
    let sym = func_symbol(func);
    let mut out = String::new();

    // Constructor: allocate the future frame, store (and retain) the params into their slots, enqueue
    // the first poll, and hand the frame back to the caller as the task handle.
    if debug {
        let _ = writeln!(out, "(func ${sym} (@name \"{}\")", func.name);
    } else {
        let _ = writeln!(out, "(func ${sym}");
    }
    for p in &body.params {
        let name = &body.locals[p.0 as usize].name;
        if debug && name.is_some() {
            let _ = writeln!(
                out,
                " (param ${} (@name \"{}\") {})",
                p.0,
                name.as_ref().unwrap(),
                wasm_ty_of(interner, body.locals[p.0 as usize].ty)
            );
        } else {
            let _ = writeln!(
                out,
                " (param ${} {})",
                p.0,
                wasm_ty_of(interner, body.locals[p.0 as usize].ty)
            );
        }
    }
    out.push_str(" (result i32)\n (local $self i32)\n");
    let _ = writeln!(out, " i32.const {frame_size}");
    let _ = writeln!(out, " i32.const {poll_idx}");
    let _ = writeln!(out, " i32.const {KIND_TASK}");
    out.push_str(" call $dream_new_future\n local.set $self\n");
    for p in &body.params {
        let idx = p.0 as usize;
        let off = slots.offsets[&idx];
        let wt = wasm_ty_of(interner, body.locals[idx].ty);
        if interner.is_reference(body.locals[idx].ty) {
            let _ = writeln!(out, " local.get ${idx}");
            out.push_str(" call $retain\n");
        }
        let _ = writeln!(
            out,
            " local.get $self\n local.get ${idx}\n {} offset={off}",
            slot_store(wt)
        );
    }
    out.push_str(" local.get $self\n call $dream_enqueue\n local.get $self\n)\n\n");

    // Poll: the coroutine state machine over `body`'s CFG. Only the persistent user locals (params +
    // declared `let`s, which lead `body.locals`) are released on completion.
    let user_local_count = hir.params.len() + hir.locals.len();
    out.push_str(&emit_async_poll(
        &body,
        interner,
        symbols,
        layouts,
        strings,
        tags,
        ftable,
        &slots,
        &poll_symbol(func),
        user_local_count,
        debug,
        debug_fn,
    ));
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
    let _ = writeln!(
        out,
        "\n call ${entry_sym}\n drop\n call $dream_run_loop\n)\n"
    );
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
