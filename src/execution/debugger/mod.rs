//! An interactive source-level debugger for Dream programs running under wasmtime, exposed to editors
//! through the Debug Adapter Protocol (DAP) over stdio.
//!
//! The compiler, when built with `-g`/`--debug-info`, instruments each function with
//! `dream_debug.enter/line/exit` host hooks and spills every named local into a pool of exported
//! `i64` globals at each statement boundary (see [`crate::mir::emit::debug_map`]). This module loads
//! the emitted `.dbg.json` [source map](sourcemap), runs the program on a dedicated thread, and lets
//! those hooks pause execution at breakpoints / steps. While paused it snapshots the current frame's
//! locals so the DAP main thread can answer `stackTrace`/`scopes`/`variables`/`evaluate` requests.
//!
//! Every wasm execution thread — the main instance (`threadId` 1) and each spawned `WebWorker` — is
//! surfaced as its own DAP thread with an independent [`state::ThreadState`]; the hooks are tagged
//! with the thread id they run on, so workers stop/resume and report call stacks and variables
//! independently. Breakpoints are shared across threads.
//!
//! ```text
//!  client (VS Code) <--DAP/stdio--> main thread <--Shared+Condvar--> {main, worker…} wasm threads
//! ```
//!
//! Split by concern:
//! - [`requests`]: handlers for the read-only DAP requests (`threads`, `setBreakpoints`,
//!   `stackTrace`, `scopes`, `variables`, `evaluate`).
//! - [`runner`]: spawns and runs the wasm program under the debug hooks, and attaches worker threads.
//! - [`decode`]: decodes a paused thread's locals out of linear memory into DAP `variables` trees.

mod decode;
mod protocol;
mod requests;
mod runner;
mod sourcemap;
mod state;

use protocol::{read_message, DapWriter};
use requests::{
    handle_evaluate, handle_set_breakpoints, handle_stack_trace, handle_threads, handle_variables,
    scopes_body,
};
use runner::{spawn_execution, WorkerAttach};
use serde_json::{json, Value};
use sourcemap::SourceMap;
use state::{RunMode, Shared};
use std::io::{self, Stdout};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

type Writer = Arc<Mutex<DapWriter<Stdout>>>;

/// The DAP thread id of the main wasm instance. Workers are assigned `worker_registry_id + 1`.
const MAIN_THREAD: u32 = 1;

/// A human-readable name for a DAP thread id (`main`, or `worker N`).
fn thread_name(thread_id: u32) -> String {
    if thread_id == MAIN_THREAD {
        "main".to_string()
    } else {
        format!("worker {}", thread_id - 1)
    }
}

/// Entry point for the `dream debug-adapter <file>` subcommand: `wat_path` is the compiled `.wat`
/// (its `.dbg.json` sibling holds the source map). Speaks DAP over stdin/stdout until the client
/// disconnects.
pub fn run_debug_adapter(wat_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let map_path = debug_map_path(wat_path);
    let source_map = Arc::new(SourceMap::load(&map_path).map_err(io_err)?);
    let shared = Arc::new(Shared::default());
    let writer: Writer = Arc::new(Mutex::new(DapWriter::new(io::stdout())));

    // Attach the debugger to worker threads: each spawned worker becomes its own DAP thread with the
    // real debug hooks linked into its instance (see `WorkerAttach`).
    crate::execution::host::set_worker_debug(Arc::new(WorkerAttach {
        shared: shared.clone(),
        source_map: source_map.clone(),
        writer: writer.clone(),
    }));

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut wasm_thread: Option<JoinHandle<()>> = None;
    let mut stop_on_entry = false;

    while let Some(msg) = read_message(&mut reader)? {
        let command = msg
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        match command.as_str() {
            "initialize" => {
                writer.lock().unwrap().respond(&msg, capabilities())?;
                writer.lock().unwrap().event("initialized", json!({}))?;
            }
            "launch" => {
                // `stopOnEntry` makes the main thread pause at its first line.
                stop_on_entry = msg
                    .pointer("/arguments/stopOnEntry")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "setBreakpoints" => {
                let body = handle_set_breakpoints(&msg, &shared, &source_map);
                writer.lock().unwrap().respond(&msg, body)?;
            }
            "setExceptionBreakpoints" => {
                writer
                    .lock()
                    .unwrap()
                    .respond(&msg, json!({ "breakpoints": [] }))?;
            }
            "configurationDone" => {
                writer.lock().unwrap().respond(&msg, Value::Null)?;
                // Start execution only once (guard against duplicate configurationDone).
                if wasm_thread.is_none() {
                    wasm_thread = Some(spawn_execution(
                        wat_path.to_string(),
                        shared.clone(),
                        source_map.clone(),
                        writer.clone(),
                        stop_on_entry,
                    ));
                }
            }
            "threads" => {
                let body = handle_threads(&shared);
                writer.lock().unwrap().respond(&msg, body)?;
            }
            "stackTrace" => {
                let tid = arg_thread_id(&msg);
                let body = handle_stack_trace(&shared, &source_map, tid);
                writer.lock().unwrap().respond(&msg, body)?;
            }
            "scopes" => {
                let frame_id = msg
                    .pointer("/arguments/frameId")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                writer
                    .lock()
                    .unwrap()
                    .respond(&msg, scopes_body(frame_id))?;
            }
            "variables" => {
                let reference = msg
                    .pointer("/arguments/variablesReference")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let body = handle_variables(&shared, reference);
                writer.lock().unwrap().respond(&msg, body)?;
            }
            "evaluate" => {
                let body = handle_evaluate(&msg, &shared);
                writer.lock().unwrap().respond(&msg, body)?;
            }
            "continue" => {
                let tid = arg_thread_id(&msg);
                resume(&shared, tid, RunMode::Continue);
                writer
                    .lock()
                    .unwrap()
                    .respond(&msg, json!({ "allThreadsContinued": false }))?;
            }
            "next" => {
                let tid = arg_thread_id(&msg);
                let depth = thread_depth(&shared, tid);
                resume(&shared, tid, RunMode::StepOver(depth));
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "stepIn" => {
                let tid = arg_thread_id(&msg);
                resume(&shared, tid, RunMode::StepIn);
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "stepOut" => {
                let tid = arg_thread_id(&msg);
                let depth = thread_depth(&shared, tid);
                resume(&shared, tid, RunMode::StepOut(depth));
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "pause" => {
                let tid = arg_thread_id(&msg);
                if let Some(t) = shared.inner.lock().unwrap().threads.get_mut(&tid) {
                    t.pause_requested = true;
                }
                // Arm the lock-free flag so the next line hook on that thread escalates and stops.
                if let Some(hot) = shared.hot_get(tid) {
                    hot.pause.store(true, Ordering::Relaxed);
                }
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "disconnect" | "terminate" => {
                writer.lock().unwrap().respond(&msg, Value::Null)?;
                // Release every paused thread so all wasm threads can unwind and exit.
                resume_all(&shared);
                break;
            }
            _ => {
                // Unknown/unsupported request: acknowledge so the client is not left waiting.
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
        }
    }

    // Best-effort: let the execution thread finish if it is still running.
    if let Some(handle) = wasm_thread {
        let _ = handle.join();
    }
    Ok(())
}

/// The DAP capabilities we advertise on `initialize`.
fn capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "supportsEvaluateForHovers": true,
        "supportsTerminateRequest": true,
        "supportsStepInTargetsRequest": false,
        "supportsSetVariable": false,
    })
}

/// The `threadId` argument of a request, defaulting to the main thread.
fn arg_thread_id(msg: &Value) -> u32 {
    msg.pointer("/arguments/threadId")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(MAIN_THREAD)
}

/// The current call depth of a thread (for `StepOver`/`StepOut` reference depths).
fn thread_depth(shared: &Arc<Shared>, thread_id: u32) -> usize {
    shared
        .inner
        .lock()
        .unwrap()
        .threads
        .get(&thread_id)
        .map(|t| t.call_stack.len())
        .unwrap_or(0)
}

/// Sets a thread's run mode and releases its paused hook (if any). Mirrors the mode into the
/// lock-free hot state so the fast path enforces it without locking.
fn resume(shared: &Arc<Shared>, thread_id: u32, mode: RunMode) {
    {
        let mut inner = shared.inner.lock().unwrap();
        if let Some(t) = inner.threads.get_mut(&thread_id) {
            t.mode = mode;
            t.resume = true;
        }
    }
    if let Some(hot) = shared.hot_get(thread_id) {
        hot.set_mode(mode);
    }
    shared.cv.notify_all();
}

/// Releases every thread (used on disconnect so the whole program can unwind).
fn resume_all(shared: &Arc<Shared>) {
    {
        let mut inner = shared.inner.lock().unwrap();
        for t in inner.threads.values_mut() {
            t.mode = RunMode::Continue;
            t.resume = true;
        }
    }
    for hot in shared.hot.lock().unwrap().values() {
        hot.set_mode(RunMode::Continue);
    }
    shared.cv.notify_all();
}

/// Derives the `.dbg.json` path sitting next to a compiled `.wat` output.
fn debug_map_path(wat_path: &str) -> String {
    let path = std::path::Path::new(wat_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    parent
        .join(format!("{}.dbg.json", stem))
        .to_string_lossy()
        .into_owned()
}

fn io_err(msg: String) -> Box<dyn std::error::Error> {
    Box::new(io::Error::new(io::ErrorKind::Other, msg))
}
