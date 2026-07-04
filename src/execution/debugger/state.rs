//! Shared, thread-safe state coordinating the DAP main thread and the (possibly many) wasm execution
//! threads.
//!
//! The main wasm instance and every `WebWorker` run on their own OS thread, each a fresh instance of
//! the same module with its own linear memory. The compiler-inserted `dream_debug.enter/line/exit`
//! host hooks call into this state on whichever thread they run. Each execution thread is surfaced to
//! the client as a DAP *thread* with its own [`ThreadState`] (call stack, run mode, pause flags, and
//! decoded-variable snapshot); breakpoints are shared. When a hook decides to pause it snapshots that
//! thread's frame, marks the thread paused, and blocks on the shared condvar until the main thread
//! (servicing the client's per-thread `continue`/`next`/... requests) releases it.

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex};

/// Reference-namespacing stride. A thread `tid` owns `variablesReference`/`frameId` values in
/// `[tid * VAR_REF_BASE, (tid + 1) * VAR_REF_BASE)`: `tid * VAR_REF_BASE` is its top-frame Locals
/// scope, children are allocated upward from there. This lets `variables(ref)` recover the owning
/// thread by integer division without any global reference table.
pub const VAR_REF_BASE: i64 = 1_000_000;

/// How execution should proceed after a stop. `StepOver`/`StepOut` carry the call-stack depth at the
/// moment the step was requested, so the next stop can be gated on returning to that depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Run freely; stop only at breakpoints or an explicit pause.
    Continue,
    /// Stop at the very next source line, at any call depth.
    StepIn,
    /// Stop at the next line whose depth is `<=` the reference depth (same frame or after a return).
    StepOver(usize),
    /// Stop at the next line whose depth is strictly `<` the reference depth (after this frame returns).
    StepOut(usize),
}

/// One live call-stack frame, as tracked by the enter/line/exit hooks.
#[derive(Debug, Clone)]
pub struct FrameState {
    pub func_id: u32,
    pub file: u32,
    pub line: u32,
}

/// A decoded variable captured at a stop, ready to serialize into a DAP `variables` response. Rich
/// values (structs, unions, arrays) are expandable: `variables_reference` is non-zero and its
/// children are registered in the owning thread's [`ThreadState::var_refs`] under that reference.
#[derive(Debug, Clone)]
pub struct VarValue {
    pub name: String,
    pub value: String,
    /// The variable's type tag, for display (e.g. `int`, `string`, `Shape`).
    pub type_name: String,
    /// Non-zero when this variable can be expanded to reveal child fields/elements.
    pub variables_reference: i64,
}

/// Why the debuggee last stopped (mapped to the DAP `stopped` event `reason`).
#[derive(Debug, Clone, Copy)]
pub enum StopReason {
    Breakpoint,
    Step,
    Pause,
    #[allow(dead_code)]
    Entry,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Breakpoint => "breakpoint",
            StopReason::Step => "step",
            StopReason::Pause => "pause",
            StopReason::Entry => "entry",
        }
    }
}

/// Per-thread debug state. One exists for the main instance (`threadId` 1) and one per live worker.
#[derive(Debug)]
pub struct ThreadState {
    /// Display name shown in the client's thread/call-stack UI (`main`, `worker N`).
    pub name: String,
    pub mode: RunMode,
    /// Set by an explicit client `pause`; consumed at the next line hook on this thread.
    pub pause_requested: bool,
    /// The live call stack; `call_stack.last()` is the innermost (current) frame.
    pub call_stack: Vec<FrameState>,
    /// True while this thread is blocked inside a line hook waiting to resume.
    pub paused: bool,
    /// Toggled by the main thread to release this thread's paused hook.
    pub resume: bool,
    /// Decoded variable trees at the current stop, keyed by `variablesReference` (all within this
    /// thread's `[base, base + VAR_REF_BASE)` range; `base` holds the top-frame locals). Rebuilt on
    /// each stop.
    pub var_refs: HashMap<i64, Vec<VarValue>>,
}

impl ThreadState {
    pub fn new(name: impl Into<String>) -> Self {
        ThreadState {
            name: name.into(),
            mode: RunMode::Continue,
            pause_requested: false,
            call_stack: Vec::new(),
            paused: false,
            resume: false,
            var_refs: HashMap::new(),
        }
    }

    /// The base `variablesReference`/`frameId` for a thread id (its top-frame Locals scope).
    pub fn base_ref(thread_id: u32) -> i64 {
        thread_id as i64 * VAR_REF_BASE
    }
}

#[derive(Debug, Default)]
pub struct Inner {
    /// Active breakpoints as `(file_id, line)` pairs, resolved against the source map. Shared by all
    /// threads (they run the same module).
    pub breakpoints: HashSet<(u32, u32)>,
    /// Live execution threads keyed by DAP `threadId` (1 = the main instance).
    pub threads: HashMap<u32, ThreadState>,
    /// True once the main wasm program has finished (normally or via a trap).
    pub terminated: bool,
}

/// The condvar-guarded shared state handed to every thread (via `Arc`).
#[derive(Default)]
pub struct Shared {
    pub inner: Mutex<Inner>,
    pub cv: Condvar,
}

impl Shared {
    /// Decides whether a line hook on thread `t` should stop, given the active mode and call depth.
    /// Consumes a pending explicit pause. Called with the lock held; `breakpoints` and `t` are
    /// disjoint borrows of [`Inner`].
    pub fn should_stop(
        breakpoints: &HashSet<(u32, u32)>,
        t: &mut ThreadState,
    ) -> Option<StopReason> {
        let depth = t.call_stack.len();
        let at = t
            .call_stack
            .last()
            .map(|f| (f.file, f.line))
            .unwrap_or((u32::MAX, 0));
        if breakpoints.contains(&at) {
            return Some(StopReason::Breakpoint);
        }
        if t.pause_requested {
            t.pause_requested = false;
            return Some(StopReason::Pause);
        }
        match t.mode {
            RunMode::Continue => None,
            RunMode::StepIn => Some(StopReason::Step),
            RunMode::StepOver(d) => (depth <= d).then_some(StopReason::Step),
            RunMode::StepOut(d) => (depth < d).then_some(StopReason::Step),
        }
    }
}
