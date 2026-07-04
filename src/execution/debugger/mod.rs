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

mod protocol;
mod sourcemap;
mod state;

use crate::execution::host::{read_string_from_memory, WorkerDebug};
use crate::execution::wasm_runner::link_runtime_host_functions;
use protocol::{read_message, DapWriter};
use serde_json::{json, Value};
use sourcemap::{ScalarKind, SourceMap, TypeDesc};
use state::{FrameState, Inner, RunMode, Shared, ThreadState, VarValue, VAR_REF_BASE};
use std::collections::HashMap;
use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use wasmtime::*;

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

/// Lists every live execution thread. Falls back to a lone `main` before execution has started.
fn handle_threads(shared: &Arc<Shared>) -> Value {
    let inner = shared.inner.lock().unwrap();
    let mut threads: Vec<(u32, String)> = inner
        .threads
        .iter()
        .map(|(id, t)| (*id, t.name.clone()))
        .collect();
    drop(inner);
    if threads.is_empty() {
        threads.push((MAIN_THREAD, "main".to_string()));
    }
    threads.sort_by_key(|(id, _)| *id);
    let list: Vec<Value> = threads
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    json!({ "threads": list })
}

/// Sets a thread's run mode and releases its paused hook (if any).
fn resume(shared: &Arc<Shared>, thread_id: u32, mode: RunMode) {
    {
        let mut inner = shared.inner.lock().unwrap();
        if let Some(t) = inner.threads.get_mut(&thread_id) {
            t.mode = mode;
            t.resume = true;
        }
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
    shared.cv.notify_all();
}

fn handle_set_breakpoints(msg: &Value, shared: &Arc<Shared>, sm: &SourceMap) -> Value {
    let path = msg
        .pointer("/arguments/source/path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let lines: Vec<u32> = msg
        .pointer("/arguments/breakpoints")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|b| b.get("line").and_then(|l| l.as_u64()).map(|l| l as u32))
                .collect()
        })
        .unwrap_or_default();

    let file_id = sm.file_id_for_path(path);
    let mut inner = shared.inner.lock().unwrap();
    // Replace all breakpoints for this file with the new set.
    if let Some(fid) = file_id {
        inner.breakpoints.retain(|(f, _)| *f != fid);
        for &line in &lines {
            inner.breakpoints.insert((fid, line));
        }
    }
    drop(inner);

    let verified = file_id.is_some();
    let breakpoints: Vec<Value> = lines
        .iter()
        .map(|l| json!({ "verified": verified, "line": l }))
        .collect();
    json!({ "breakpoints": breakpoints })
}

fn handle_stack_trace(shared: &Arc<Shared>, sm: &SourceMap, thread_id: u32) -> Value {
    let inner = shared.inner.lock().unwrap();
    let Some(t) = inner.threads.get(&thread_id) else {
        return json!({ "stackFrames": [], "totalFrames": 0 });
    };
    let base = ThreadState::base_ref(thread_id);
    // DAP wants the innermost frame first; our call stack has the outermost first. Frame ids are
    // namespaced per thread (`base + index`) so `scopes`/`variables` never collide across threads.
    let frames: Vec<Value> = t
        .call_stack
        .iter()
        .rev()
        .enumerate()
        .map(|(i, f)| {
            let name = sm
                .function(f.func_id)
                .map(|fi| fi.name.clone())
                .unwrap_or_else(|| format!("fn#{}", f.func_id));
            let mut frame = json!({
                "id": base + i as i64,
                "name": name,
                "line": f.line,
                "column": 1,
            });
            if let Some(path) = sm.file_path(f.file) {
                frame["source"] = json!({
                    "name": std::path::Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(path),
                    "path": path,
                });
            }
            frame
        })
        .collect();
    let total = frames.len();
    json!({ "stackFrames": frames, "totalFrames": total })
}

/// Resolves a frame's Locals scope. A frame id is `thread_base + frame_index`; only the innermost
/// frame (index 0, i.e. `frame_id % VAR_REF_BASE == 0`) has live locals — its `variablesReference` is
/// the thread's base. Non-top frames return 0 (their spilled globals have been overwritten).
fn scopes_body(frame_id: i64) -> Value {
    let reference = if frame_id > 0 && frame_id % VAR_REF_BASE == 0 {
        frame_id
    } else {
        0
    };
    json!({
        "scopes": [
            {
                "name": "Locals",
                "variablesReference": reference,
                "expensive": false,
            }
        ]
    })
}

fn handle_variables(shared: &Arc<Shared>, reference: i64) -> Value {
    // The owning thread is recovered from the reference's high part (see `VAR_REF_BASE`).
    let thread_id = (reference / VAR_REF_BASE) as u32;
    let inner = shared.inner.lock().unwrap();
    let variables: Vec<Value> = inner
        .threads
        .get(&thread_id)
        .and_then(|t| t.var_refs.get(&reference))
        .map(|vs| vs.iter().map(var_to_json).collect())
        .unwrap_or_default();
    json!({ "variables": variables })
}

fn var_to_json(v: &VarValue) -> Value {
    json!({
        "name": v.name,
        "value": v.value,
        "type": v.type_name,
        "variablesReference": v.variables_reference,
    })
}

fn handle_evaluate(msg: &Value, shared: &Arc<Shared>) -> Value {
    let expr = msg
        .pointer("/arguments/expression")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    // Evaluate against the requested frame's thread (default: main). `frameId` is `thread_base + idx`.
    let frame_id = msg
        .pointer("/arguments/frameId")
        .and_then(|v| v.as_i64())
        .unwrap_or(VAR_REF_BASE);
    let thread_id = (frame_id / VAR_REF_BASE).max(1) as u32;
    let base = ThreadState::base_ref(thread_id);

    let inner = shared.inner.lock().unwrap();
    let Some(t) = inner.threads.get(&thread_id) else {
        return json!({ "result": "<not available>", "variablesReference": 0 });
    };
    // Support simple dotted field paths (`a.field.sub`) by walking the decoded variable tree.
    let mut parts = expr.split('.');
    let Some(head) = parts.next() else {
        return json!({ "result": "<not available>", "variablesReference": 0 });
    };
    let mut current = t
        .var_refs
        .get(&base)
        .and_then(|top| top.iter().find(|v| v.name == head));
    for seg in parts {
        current = match current {
            Some(v) if v.variables_reference != 0 => t
                .var_refs
                .get(&v.variables_reference)
                .and_then(|children| children.iter().find(|c| c.name == seg)),
            _ => None,
        };
    }
    match current {
        Some(v) => json!({
            "result": v.value,
            "type": v.type_name,
            "variablesReference": v.variables_reference,
        }),
        None => json!({ "result": "<not available>", "variablesReference": 0 }),
    }
}

/// Spawns the wasm execution thread: builds the engine/linker (with the debug hooks wired to
/// `shared`), instantiates the module, and runs `main`. Sends `exited`/`terminated` DAP events when
/// the program finishes.
fn spawn_execution(
    wat_path: String,
    shared: Arc<Shared>,
    source_map: Arc<SourceMap>,
    writer: Writer,
    stop_on_entry: bool,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // Register the main instance as DAP thread 1 (with a StepIn mode if `stopOnEntry`) before any
        // hook fires, and announce it to the client.
        {
            let mut inner = shared.inner.lock().unwrap();
            let t = inner
                .threads
                .entry(MAIN_THREAD)
                .or_insert_with(|| ThreadState::new("main"));
            if stop_on_entry {
                t.mode = RunMode::StepIn;
            }
        }
        let _ = writer.lock().unwrap().event(
            "thread",
            json!({ "reason": "started", "threadId": MAIN_THREAD }),
        );

        let result = run_program(&wat_path, &shared, &source_map, &writer);
        shared.inner.lock().unwrap().terminated = true;
        let mut w = writer.lock().unwrap();
        if let Err(e) = &result {
            let _ = w.event(
                "output",
                json!({ "category": "stderr", "output": format!("Program terminated: {}\n", e) }),
            );
        }
        let _ = w.event("exited", json!({ "exitCode": 0 }));
        let _ = w.event("terminated", json!({}));
    })
}

/// Builds the instrumented module and runs its entry point under the debug hooks.
fn run_program(
    wat_path: &str,
    shared: &Arc<Shared>,
    source_map: &Arc<SourceMap>,
    writer: &Writer,
) -> Result<()> {
    crate::execution::host::enable_ansi_support();
    let wat_content = std::fs::read_to_string(wat_path)
        .map_err(|e| Error::msg(format!("failed to read {}: {}", wat_path, e)))?;
    let wasm_bytes = wat::parse_str(&wat_content)?;
    crate::execution::host::set_worker_module(&wasm_bytes);

    let engine = Engine::default();
    let module = Module::new(&engine, &wasm_bytes)?;
    let mut store = Store::new(&engine, ());
    let mut linker: Linker<()> = Linker::new(&engine);

    // Program output is routed to DAP `output` events (stdout is reserved for the DAP stream).
    link_debug_print_functions(&mut linker, writer)?;
    link_runtime_host_functions(&mut linker)?;
    link_debug_hooks(&mut linker, shared, source_map, writer, MAIN_THREAD)?;
    linker.define_unknown_imports_as_traps(&module)?;

    let instance = linker.instantiate(&mut store, &module)?;
    if let Ok(main_func) = instance.get_typed_func::<(), ()>(&mut store, crate::mir::abi::ENTRY_FN)
    {
        main_func.call(&mut store, ())?;
    }
    Ok(())
}

/// The debugger's attachment to worker threads (installed via [`crate::execution::host::set_worker_debug`]).
/// Each spawned worker is mapped to `worker_registry_id + 1` and, when it starts, registered as its own
/// DAP thread with the real debug hooks + DAP-routed output linked into its instance.
struct WorkerAttach {
    shared: Arc<Shared>,
    source_map: Arc<SourceMap>,
    writer: Writer,
}

impl WorkerDebug for WorkerAttach {
    fn dap_thread_id(&self, worker_id: u32) -> u32 {
        worker_id + 1
    }

    fn on_start(&self, thread_id: u32) {
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner
                .threads
                .entry(thread_id)
                .or_insert_with(|| ThreadState::new(thread_name(thread_id)));
        }
        let _ = self.writer.lock().unwrap().event(
            "thread",
            json!({ "reason": "started", "threadId": thread_id }),
        );
    }

    fn on_exit(&self, thread_id: u32) {
        {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.threads.remove(&thread_id);
        }
        // Wake anyone (e.g. the main thread's condvar waiters) and notify the client.
        self.shared.cv.notify_all();
        let _ = self.writer.lock().unwrap().event(
            "thread",
            json!({ "reason": "exited", "threadId": thread_id }),
        );
    }

    fn install(&self, linker: &mut Linker<()>, thread_id: u32) {
        let _ = link_debug_print_functions(linker, &self.writer);
        let _ = link_debug_hooks(
            linker,
            &self.shared,
            &self.source_map,
            &self.writer,
            thread_id,
        );
    }
}

/// The `print_*`/`println` builtins wired to DAP `output` events instead of process stdout.
fn link_debug_print_functions(linker: &mut Linker<()>, writer: &Writer) -> Result<()> {
    let emit = |writer: &Writer, text: String| {
        let _ = writer
            .lock()
            .unwrap()
            .event("output", json!({ "category": "stdout", "output": text }));
    };

    let w = writer.clone();
    linker.func_wrap("env", "print_int", move |v: i32| emit(&w, v.to_string()))?;
    let w = writer.clone();
    linker.func_wrap("env", "print_float", move |v: f32| emit(&w, v.to_string()))?;
    let w = writer.clone();
    linker.func_wrap("env", "print_double", move |v: f64| emit(&w, v.to_string()))?;
    let w = writer.clone();
    linker.func_wrap("env", "print_char", move |v: i32| {
        if let Some(c) = char::from_u32(v as u32) {
            emit(&w, c.to_string());
        }
    })?;
    let w = writer.clone();
    linker.func_wrap(
        "env",
        "print_string",
        move |mut caller: Caller<'_, ()>, ptr: i32| -> Result<()> {
            let s = read_caller_string(&mut caller, ptr)?;
            emit(&w, s);
            Ok(())
        },
    )?;
    let w = writer.clone();
    linker.func_wrap(
        "env",
        "println",
        move |mut caller: Caller<'_, ()>, ptr: i32| -> Result<()> {
            let s = read_caller_string(&mut caller, ptr)?;
            emit(&w, format!("{}\n", s));
            Ok(())
        },
    )?;
    Ok(())
}

/// Wires the compiler-inserted `dream_debug.enter/line/exit` hooks into `linker`, closing over the
/// shared debug state and the DAP `thread_id` this instance runs as. Line hooks operate only on that
/// thread's [`ThreadState`], so the main instance and each worker stop/resume independently.
fn link_debug_hooks(
    linker: &mut Linker<()>,
    shared: &Arc<Shared>,
    source_map: &Arc<SourceMap>,
    writer: &Writer,
    thread_id: u32,
) -> Result<()> {
    let sh = shared.clone();
    linker.func_wrap(
        "dream_debug",
        "enter",
        move |_c: Caller<'_, ()>, id: i32| {
            let mut inner = sh.inner.lock().unwrap();
            let t = inner
                .threads
                .entry(thread_id)
                .or_insert_with(|| ThreadState::new(thread_name(thread_id)));
            t.call_stack.push(FrameState {
                func_id: id as u32,
                file: 0,
                line: 0,
            });
        },
    )?;

    let sh = shared.clone();
    linker.func_wrap(
        "dream_debug",
        "exit",
        move |_c: Caller<'_, ()>, _id: i32| {
            let mut inner = sh.inner.lock().unwrap();
            if let Some(t) = inner.threads.get_mut(&thread_id) {
                t.call_stack.pop();
            }
        },
    )?;

    let sh = shared.clone();
    let sm = source_map.clone();
    let wr = writer.clone();
    linker.func_wrap(
        "dream_debug",
        "line",
        move |mut caller: Caller<'_, ()>, file_id: i32, line: i32| {
            let stop = {
                let mut inner = sh.inner.lock().unwrap();
                let Inner {
                    breakpoints,
                    threads,
                    ..
                } = &mut *inner;
                let Some(t) = threads.get_mut(&thread_id) else {
                    return;
                };
                if let Some(frame) = t.call_stack.last_mut() {
                    frame.file = file_id as u32;
                    frame.line = line as u32;
                }
                Shared::should_stop(breakpoints, t)
            };
            let Some(reason) = stop else {
                return;
            };
            // Snapshot this thread's current frame (and expandable children) while we still hold the
            // wasm caller; references are namespaced in the thread's range.
            let var_refs = snapshot_locals(&mut caller, &sh, &sm, thread_id);
            {
                let mut inner = sh.inner.lock().unwrap();
                if let Some(t) = inner.threads.get_mut(&thread_id) {
                    t.var_refs = var_refs;
                    t.paused = true;
                    t.resume = false;
                }
            }
            let _ = wr.lock().unwrap().event(
                "stopped",
                json!({
                    "reason": reason.as_str(),
                    "threadId": thread_id,
                    "allThreadsStopped": false,
                }),
            );
            // Park until the client resumes *this* thread (continue/step/disconnect).
            let mut inner = sh.inner.lock().unwrap();
            loop {
                match inner.threads.get(&thread_id) {
                    Some(t) if t.resume => break,
                    None => return,
                    _ => {}
                }
                inner = sh.cv.wait(inner).unwrap();
            }
            if let Some(t) = inner.threads.get_mut(&thread_id) {
                t.paused = false;
            }
        },
    )?;
    Ok(())
}

/// Maximum recursion depth when eagerly decoding an aggregate into an inline summary. Nested values
/// beyond this are collapsed to `{…}` but remain expandable on demand from the summary's children.
const MAX_DECODE_DEPTH: usize = 4;
/// Caps on how much of a container we materialize, to bound work on large/cyclic data.
const MAX_FIELDS: usize = 128;
const MAX_ELEMS: usize = 200;

/// Reads and decodes every named local of `thread_id`'s innermost frame from the spill-pool globals,
/// walking linear memory to expand strings, structs, unions, and arrays. Returns the thread's stop-time
/// `variablesReference -> children` map, keyed within the thread's range: the thread base holds the
/// top-frame locals, expandable children get base+1.. . An empty map means nothing to show.
fn snapshot_locals(
    caller: &mut Caller<'_, ()>,
    shared: &Arc<Shared>,
    sm: &SourceMap,
    thread_id: u32,
) -> HashMap<i64, Vec<VarValue>> {
    let base = ThreadState::base_ref(thread_id);
    let func_id = {
        let inner = shared.inner.lock().unwrap();
        inner
            .threads
            .get(&thread_id)
            .and_then(|t| t.call_stack.last().map(|f| f.func_id))
    };
    let Some(info) = func_id.and_then(|id| sm.function(id)) else {
        return HashMap::new();
    };

    // Read the raw spilled slots first (needs `&mut caller`), then decode against memory.
    let raws: Vec<(String, u32, u64)> = info
        .vars
        .iter()
        .map(|v| {
            (
                v.name.clone(),
                v.type_id,
                read_global_i64(caller, v.global).unwrap_or(0) as u64,
            )
        })
        .collect();

    let Some(mem) = caller.get_export("memory").and_then(Extern::into_memory) else {
        return HashMap::new();
    };
    let mut dec = Decoder {
        caller: &*caller,
        mem,
        types: &sm.types,
        refs: HashMap::new(),
        next_ref: base + 1,
    };
    let mut locals = Vec::with_capacity(raws.len());
    for (name, type_id, raw) in raws {
        let (value, type_name, vref) = dec.build(type_id, raw, MAX_DECODE_DEPTH);
        locals.push(VarValue {
            name,
            value,
            type_name,
            variables_reference: vref,
        });
    }
    // The top-frame locals live under the thread's base reference (returned by `scopes`).
    dec.refs.insert(base, locals);
    dec.refs
}

/// Reads the raw `i64` bits spilled into the `$__dbg_v{global}` pool global.
fn read_global_i64(caller: &mut Caller<'_, ()>, global: u32) -> Option<i64> {
    let name = format!("__dbg_v{}", global);
    let g = caller.get_export(&name).and_then(Extern::into_global)?;
    match g.get(&mut *caller) {
        Val::I64(v) => Some(v),
        _ => None,
    }
}

/// Walks live values in linear memory against the debug type table, producing displayable strings and
/// a registry of expandable children keyed by `variablesReference`.
struct Decoder<'a, 'c> {
    caller: &'a Caller<'c, ()>,
    mem: Memory,
    types: &'a [TypeDesc],
    refs: HashMap<i64, Vec<VarValue>>,
    next_ref: i64,
}

impl Decoder<'_, '_> {
    fn desc(&self, type_id: u32) -> TypeDesc {
        self.types
            .get(type_id as usize)
            .cloned()
            .unwrap_or(TypeDesc::Ref)
    }

    /// Registers a child list and returns a fresh `variablesReference` for it (0 if empty).
    fn alloc(&mut self, children: Vec<VarValue>) -> i64 {
        if children.is_empty() {
            return 0;
        }
        let r = self.next_ref;
        self.next_ref += 1;
        self.refs.insert(r, children);
        r
    }

    fn u8_at(&self, addr: u32) -> u8 {
        let d = self.mem.data(self.caller);
        d.get(addr as usize).copied().unwrap_or(0)
    }

    fn u32_at(&self, addr: u32) -> u32 {
        let d = self.mem.data(self.caller);
        let a = addr as usize;
        if a + 4 <= d.len() {
            u32::from_le_bytes([d[a], d[a + 1], d[a + 2], d[a + 3]])
        } else {
            0
        }
    }

    fn u64_at(&self, addr: u32) -> u64 {
        let d = self.mem.data(self.caller);
        let a = addr as usize;
        if a + 8 <= d.len() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&d[a..a + 8]);
            u64::from_le_bytes(b)
        } else {
            0
        }
    }

    fn read_string(&self, ptr: u32) -> String {
        read_string_from_memory(&self.mem, self.caller, ptr as i32)
    }

    /// Loads a scalar's raw bits from memory at `addr`, honoring its storage width.
    fn load_scalar(&self, addr: u32, k: ScalarKind) -> u64 {
        match k.width() {
            1 => self.u8_at(addr) as u64,
            8 => self.u64_at(addr),
            _ => self.u32_at(addr) as u64,
        }
    }

    /// Computes the "raw" value for a field at `base + field.offset`: the scalar bits for a scalar, the
    /// inline address for a value aggregate, or the loaded pointer for a reference/string/array field.
    fn field_raw(&self, base: u32, field: &sourcemap::FieldDesc) -> u64 {
        let addr = base.wrapping_add(field.offset);
        match self.desc(field.type_id) {
            TypeDesc::Scalar(k) => self.load_scalar(addr, k),
            TypeDesc::Enum => self.u32_at(addr) as u64,
            TypeDesc::Str | TypeDesc::Array { .. } | TypeDesc::Ref => self.u32_at(addr) as u64,
            TypeDesc::Struct { value: true, .. } | TypeDesc::Union { value: true, .. } => {
                addr as u64
            }
            TypeDesc::Struct { .. } | TypeDesc::Union { .. } => self.u32_at(addr) as u64,
        }
    }

    /// Decodes a value of type `type_id` given its `raw` slot (scalar bits, or a data pointer in the
    /// low 32 bits for aggregates/strings). Returns `(display, type_name, variables_reference)`.
    fn build(&mut self, type_id: u32, raw: u64, depth: usize) -> (String, String, i64) {
        match self.desc(type_id) {
            TypeDesc::Scalar(k) => (decode_scalar(k, raw), k.tag().to_string(), 0),
            TypeDesc::Enum => ((raw as u32 as i32).to_string(), "enum".to_string(), 0),
            TypeDesc::Str => {
                let ptr = raw as u32;
                let val = if ptr == 0 {
                    "null".to_string()
                } else {
                    format!("\"{}\"", self.read_string(ptr))
                };
                (val, "string".to_string(), 0)
            }
            TypeDesc::Ref => {
                let low = raw as u32;
                let val = if low == 0 {
                    "null".to_string()
                } else {
                    format!("0x{:x}", low)
                };
                (val, "ref".to_string(), 0)
            }
            TypeDesc::Struct { name, fields, .. } => {
                self.build_struct(&name, &fields, raw as u32, depth)
            }
            TypeDesc::Union { name, variants, .. } => {
                self.build_union(&name, &variants, raw as u32, depth)
            }
            TypeDesc::Array { elem, stride } => self.build_array(elem, stride, raw as u32, depth),
        }
    }

    fn build_struct(
        &mut self,
        name: &str,
        fields: &[sourcemap::FieldDesc],
        ptr: u32,
        depth: usize,
    ) -> (String, String, i64) {
        if ptr == 0 {
            return ("null".to_string(), name.to_string(), 0);
        }
        if depth == 0 {
            return (format!("{} {{…}}", name), name.to_string(), 0);
        }
        let mut children = Vec::new();
        let mut parts = Vec::new();
        for field in fields.iter().take(MAX_FIELDS) {
            let raw = self.field_raw(ptr, field);
            let (value, type_name, vref) = self.build(field.type_id, raw, depth - 1);
            parts.push(format!("{}: {}", field.name, value));
            children.push(VarValue {
                name: field.name.clone(),
                value,
                type_name,
                variables_reference: vref,
            });
        }
        let summary = format!("{} {{ {} }}", name, parts.join(", "));
        let vref = self.alloc(children);
        (summary, name.to_string(), vref)
    }

    fn build_union(
        &mut self,
        name: &str,
        variants: &[sourcemap::VariantDesc],
        ptr: u32,
        depth: usize,
    ) -> (String, String, i64) {
        if ptr == 0 {
            return ("null".to_string(), name.to_string(), 0);
        }
        let disc = self.u32_at(ptr) as i32;
        let Some(variant) = variants.iter().find(|v| v.discriminant == disc) else {
            return (format!("{}(<tag {}>)", name, disc), name.to_string(), 0);
        };
        let type_name = format!("{}.{}", name, variant.name);
        if variant.fields.is_empty() {
            return (variant.name.clone(), type_name, 0);
        }
        if depth == 0 {
            return (format!("{}(…)", variant.name), type_name, 0);
        }
        let mut children = Vec::new();
        let mut parts = Vec::new();
        for field in variant.fields.iter().take(MAX_FIELDS) {
            let raw = self.field_raw(ptr, field);
            let (value, tn, vref) = self.build(field.type_id, raw, depth - 1);
            parts.push(value.clone());
            children.push(VarValue {
                name: field.name.clone(),
                value,
                type_name: tn,
                variables_reference: vref,
            });
        }
        let summary = format!("{}({})", variant.name, parts.join(", "));
        let vref = self.alloc(children);
        (summary, type_name, vref)
    }

    fn build_array(
        &mut self,
        elem: u32,
        stride: u32,
        ptr: u32,
        depth: usize,
    ) -> (String, String, i64) {
        if ptr == 0 {
            return ("null".to_string(), "array".to_string(), 0);
        }
        let len = self.u32_at(ptr);
        let elem_type = element_type_name(&self.desc(elem));
        let type_name = format!("{}[]", elem_type);
        if depth == 0 {
            return (format!("{}[{}]", elem_type, len), type_name, 0);
        }
        let shown = (len as usize).min(MAX_ELEMS);
        let mut children = Vec::new();
        for i in 0..shown {
            let addr = ptr + 4 + (i as u32) * stride;
            let raw = self.elem_raw(addr, elem);
            let (value, tn, vref) = self.build(elem, raw, depth - 1);
            children.push(VarValue {
                name: format!("[{}]", i),
                value,
                type_name: tn,
                variables_reference: vref,
            });
        }
        let summary = format!("{}[{}]", elem_type, len);
        let vref = self.alloc(children);
        (summary, type_name, vref)
    }

    /// The raw value of an array element stored at `addr` (mirrors [`Self::field_raw`]).
    fn elem_raw(&self, addr: u32, elem: u32) -> u64 {
        match self.desc(elem) {
            TypeDesc::Scalar(k) => self.load_scalar(addr, k),
            TypeDesc::Enum => self.u32_at(addr) as u64,
            TypeDesc::Str | TypeDesc::Array { .. } | TypeDesc::Ref => self.u32_at(addr) as u64,
            TypeDesc::Struct { value: true, .. } | TypeDesc::Union { value: true, .. } => {
                addr as u64
            }
            TypeDesc::Struct { .. } | TypeDesc::Union { .. } => self.u32_at(addr) as u64,
        }
    }
}

/// A short type label for an array element type, used to name the array (`int[]`, `Point[]`).
fn element_type_name(desc: &TypeDesc) -> String {
    match desc {
        TypeDesc::Scalar(k) => k.tag().to_string(),
        TypeDesc::Str => "string".to_string(),
        TypeDesc::Enum => "enum".to_string(),
        TypeDesc::Struct { name, .. } | TypeDesc::Union { name, .. } => name.clone(),
        TypeDesc::Array { .. } => "array".to_string(),
        TypeDesc::Ref => "ref".to_string(),
    }
}

/// Decodes a scalar's raw bits into its displayable form.
fn decode_scalar(kind: ScalarKind, raw: u64) -> String {
    let low = raw as u32;
    match kind {
        ScalarKind::Int => (low as i32).to_string(),
        ScalarKind::UInt => low.to_string(),
        ScalarKind::Byte => (low as u8).to_string(),
        ScalarKind::Bool => {
            if low != 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        ScalarKind::Char => char::from_u32(low)
            .map(|c| format!("'{}'", c))
            .unwrap_or_else(|| format!("\\u{{{:x}}}", low)),
        ScalarKind::Long => (raw as i64).to_string(),
        ScalarKind::ULong => raw.to_string(),
        ScalarKind::Float => f32::from_bits(low).to_string(),
        ScalarKind::Double => f64::from_bits(raw).to_string(),
    }
}

fn read_caller_string(caller: &mut Caller<'_, ()>, ptr: i32) -> Result<String> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| Error::msg("module must export `memory`"))?;
    Ok(read_string_from_memory(&memory, &*caller, ptr))
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
