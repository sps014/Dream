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
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

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

/// Step-mode encodings stored in [`ThreadHot::step_mode`] (mirrors [`RunMode`] for the lock-free path).
pub const STEP_CONTINUE: u8 = 0;
pub const STEP_IN: u8 = 1;
pub const STEP_OVER: u8 = 2;
pub const STEP_OUT: u8 = 3;

/// Number of `u64` words in the breakpoint filter (→ 64× bits).
const BP_FILTER_WORDS: usize = 1024;
const BP_FILTER_BITS: u64 = (BP_FILTER_WORDS as u64) * 64;

/// A lock-free approximate membership filter over breakpoint `(file, line)` pairs, so the per-statement
/// line hook can reject the overwhelmingly common "no breakpoint here" case with a single atomic load
/// (no mutex, no hashing of the whole set). False positives merely fall through to the exact locked
/// check; there are never false negatives, so no breakpoint is ever missed.
pub struct BpFilter {
    words: Vec<AtomicU64>,
}

impl Default for BpFilter {
    fn default() -> Self {
        BpFilter {
            words: (0..BP_FILTER_WORDS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

impl BpFilter {
    fn bit_index(file: u32, line: u32) -> u64 {
        // Cheap mix of (file, line); collisions only cost an extra locked check.
        let h = (file as u64).wrapping_mul(0x9E3779B1).wrapping_add(line as u64);
        h % BP_FILTER_BITS
    }

    /// Rebuilds the filter from the exact breakpoint set (called on every `setBreakpoints`).
    pub fn rebuild(&self, breakpoints: &HashSet<(u32, u32)>) {
        for w in &self.words {
            w.store(0, Ordering::Relaxed);
        }
        for &(file, line) in breakpoints {
            let i = Self::bit_index(file, line);
            self.words[(i / 64) as usize].fetch_or(1u64 << (i % 64), Ordering::Relaxed);
        }
    }

    /// True if `(file, line)` *might* be a breakpoint (exact check still required on the slow path).
    pub fn probe(&self, file: u32, line: u32) -> bool {
        let i = Self::bit_index(file, line);
        self.words[(i / 64) as usize].load(Ordering::Relaxed) & (1u64 << (i % 64)) != 0
    }
}

/// Lock-free per-thread state read on the hot line-hook path, so a thread running freely (no
/// breakpoint at the current line, not stepping, no pause pending) never touches the shared mutex.
/// The main thread keeps these in sync with the authoritative [`ThreadState`] when it changes a
/// thread's run mode.
#[derive(Debug, Default)]
pub struct ThreadHot {
    /// The last executed statement, packed as `(file_id << 32) | line`. Updated on every line hook so
    /// a stack trace taken at the next stop has accurate frame lines without per-line locking.
    pub pos: AtomicU64,
    /// Current call depth (incremented on `enter`, decremented on `exit`), for step gating.
    pub depth: AtomicUsize,
    /// One of `STEP_*`; drives whether the fast path must escalate to the locked stop check.
    pub step_mode: AtomicU8,
    /// The reference call depth captured when a `StepOver`/`StepOut` began.
    pub step_ref: AtomicUsize,
    /// Set when an explicit `pause` is pending for this thread.
    pub pause: AtomicBool,
}

impl ThreadHot {
    pub fn pack_pos(file: u32, line: u32) -> u64 {
        ((file as u64) << 32) | (line as u64)
    }

    /// Mirrors a [`RunMode`] into the lock-free step fields.
    pub fn set_mode(&self, mode: RunMode) {
        match mode {
            RunMode::Continue => self.step_mode.store(STEP_CONTINUE, Ordering::Relaxed),
            RunMode::StepIn => self.step_mode.store(STEP_IN, Ordering::Relaxed),
            RunMode::StepOver(d) => {
                self.step_ref.store(d, Ordering::Relaxed);
                self.step_mode.store(STEP_OVER, Ordering::Relaxed);
            }
            RunMode::StepOut(d) => {
                self.step_ref.store(d, Ordering::Relaxed);
                self.step_mode.store(STEP_OUT, Ordering::Relaxed);
            }
        }
    }

    /// Whether the current line must escalate to the locked stop check because this thread is stepping.
    pub fn step_wants_stop(&self) -> bool {
        match self.step_mode.load(Ordering::Relaxed) {
            STEP_IN => true,
            STEP_OVER => self.depth.load(Ordering::Relaxed) <= self.step_ref.load(Ordering::Relaxed),
            STEP_OUT => self.depth.load(Ordering::Relaxed) < self.step_ref.load(Ordering::Relaxed),
            _ => false,
        }
    }
}

/// The condvar-guarded shared state handed to every thread (via `Arc`).
#[derive(Default)]
pub struct Shared {
    pub inner: Mutex<Inner>,
    pub cv: Condvar,
    /// Lock-free breakpoint filter, rebuilt on `setBreakpoints`.
    pub bp_filter: BpFilter,
    /// Lock-free per-thread hot state, keyed by DAP `threadId`. Populated when a thread's hooks are
    /// linked; read on the hot path via the `Arc` each hook closure captures.
    pub hot: Mutex<HashMap<u32, Arc<ThreadHot>>>,
}

impl Shared {
    /// Returns (creating if needed) the lock-free hot state for a thread, registering it so the main
    /// thread can flip its step/pause flags.
    pub fn hot_for(&self, thread_id: u32) -> Arc<ThreadHot> {
        self.hot
            .lock()
            .unwrap()
            .entry(thread_id)
            .or_insert_with(|| Arc::new(ThreadHot::default()))
            .clone()
    }

    /// Looks up a thread's hot state without creating it.
    pub fn hot_get(&self, thread_id: u32) -> Option<Arc<ThreadHot>> {
        self.hot.lock().unwrap().get(&thread_id).cloned()
    }

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
