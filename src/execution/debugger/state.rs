//! Shared, thread-safe state coordinating the DAP main thread and the wasm execution thread.
//!
//! The wasm program runs on its own thread; the compiler-inserted `dream_debug.enter/line/exit`
//! host hooks call into this state on that thread. When a hook decides to pause (breakpoint / step /
//! explicit pause) it snapshots the current frame's locals, marks the debuggee paused, and blocks on
//! the condvar until the main thread (servicing the client's `continue`/`next`/... requests) sets a
//! new [`RunMode`] and releases it.

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex};

/// How execution should proceed after a stop. `StepOver`/`StepOut` carry the call-stack depth at the
/// moment the step was requested, so the next stop can be gated on returning to that depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Run freely; stop only at breakpoints or an explicit pause.
    Continue,
    /// Stop at the very next source line, at any call depth.
    StepIn,
    /// Stop at the next line whose depth is `<=` the reference depth (i.e. same frame or after a
    /// return), running nested calls to completion.
    StepOver(usize),
    /// Stop at the next line whose depth is strictly `<` the reference depth (after this frame
    /// returns).
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
/// children are registered in [`Inner::var_refs`] under that reference.
#[derive(Debug, Clone)]
pub struct VarValue {
    pub name: String,
    pub value: String,
    /// The variable's type tag, for display (e.g. `int`, `string`, `Shape`).
    pub type_name: String,
    /// Non-zero when this variable can be expanded to reveal child fields/elements.
    pub variables_reference: i64,
}

/// Why the reason the debuggee last stopped (mapped to the DAP `stopped` event `reason`).
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

#[derive(Debug)]
pub struct Inner {
    /// Active breakpoints as `(file_id, line)` pairs, resolved against the source map.
    pub breakpoints: HashSet<(u32, u32)>,
    pub mode: RunMode,
    /// Set by an explicit client `pause`; consumed at the next line hook.
    pub pause_requested: bool,
    /// The live call stack; `call_stack.last()` is the innermost (current) frame.
    pub call_stack: Vec<FrameState>,
    /// True while the wasm thread is blocked inside a line hook waiting to resume.
    pub paused: bool,
    /// Toggled by the main thread to release a paused hook.
    pub resume: bool,
    /// True once the wasm program has finished (normally or via a trap).
    pub terminated: bool,
    /// Decoded locals of the innermost frame at the current stop (empty while running).
    pub locals: Vec<VarValue>,
    /// Children of every expandable value decoded at the current stop, keyed by the
    /// `variablesReference` handed to the client. Rebuilt on each stop.
    pub var_refs: HashMap<i64, Vec<VarValue>>,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            breakpoints: HashSet::new(),
            mode: RunMode::Continue,
            pause_requested: false,
            call_stack: Vec::new(),
            paused: false,
            resume: false,
            terminated: false,
            locals: Vec::new(),
            var_refs: HashMap::new(),
        }
    }
}

/// The condvar-guarded shared state handed to both threads (via `Arc`).
#[derive(Default)]
pub struct Shared {
    pub inner: Mutex<Inner>,
    pub cv: Condvar,
}

impl Shared {
    /// Decides whether a line hook at the current top-of-stack should stop, given the active mode and
    /// call depth. Consumes a pending explicit pause. Called with the lock held.
    pub fn should_stop(inner: &mut Inner) -> Option<StopReason> {
        let depth = inner.call_stack.len();
        let at = inner
            .call_stack
            .last()
            .map(|f| (f.file, f.line))
            .unwrap_or((u32::MAX, 0));
        if inner.breakpoints.contains(&at) {
            return Some(StopReason::Breakpoint);
        }
        if inner.pause_requested {
            inner.pause_requested = false;
            return Some(StopReason::Pause);
        }
        match inner.mode {
            RunMode::Continue => None,
            RunMode::StepIn => Some(StopReason::Step),
            RunMode::StepOver(d) => (depth <= d).then_some(StopReason::Step),
            RunMode::StepOut(d) => (depth < d).then_some(StopReason::Step),
        }
    }
}
