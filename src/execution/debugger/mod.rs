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
//! ```text
//!  client (VS Code)  <--DAP/stdio-->  main thread  <--Shared+Condvar-->  wasm thread (hooks)
//! ```

mod protocol;
mod sourcemap;
mod state;

use crate::execution::host::read_string_from_memory;
use crate::execution::wasm_runner::link_runtime_host_functions;
use protocol::{read_message, DapWriter};
use serde_json::{json, Value};
use sourcemap::{SourceMap, VarKind};
use state::{FrameState, RunMode, Shared, VarValue};
use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use wasmtime::*;

type Writer = Arc<Mutex<DapWriter<Stdout>>>;

/// Entry point for the `dream debug-adapter <file>` subcommand: `wat_path` is the compiled `.wat`
/// (its `.dbg.json` sibling holds the source map). Speaks DAP over stdin/stdout until the client
/// disconnects.
pub fn run_debug_adapter(wat_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let map_path = debug_map_path(wat_path);
    let source_map = Arc::new(SourceMap::load(&map_path).map_err(io_err)?);
    let shared = Arc::new(Shared::default());
    let writer: Writer = Arc::new(Mutex::new(DapWriter::new(io::stdout())));

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut wasm_thread: Option<JoinHandle<()>> = None;

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
                // `stopOnEntry` makes the program pause at its first line.
                let stop_on_entry = msg
                    .pointer("/arguments/stopOnEntry")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if stop_on_entry {
                    shared.inner.lock().unwrap().mode = RunMode::StepIn;
                }
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
                    ));
                }
            }
            "threads" => {
                writer
                    .lock()
                    .unwrap()
                    .respond(&msg, json!({ "threads": [ { "id": 1, "name": "main" } ] }))?;
            }
            "stackTrace" => {
                let body = handle_stack_trace(&shared, &source_map);
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
                resume(&shared, RunMode::Continue);
                writer
                    .lock()
                    .unwrap()
                    .respond(&msg, json!({ "allThreadsContinued": true }))?;
            }
            "next" => {
                let depth = shared.inner.lock().unwrap().call_stack.len();
                resume(&shared, RunMode::StepOver(depth));
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "stepIn" => {
                resume(&shared, RunMode::StepIn);
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "stepOut" => {
                let depth = shared.inner.lock().unwrap().call_stack.len();
                resume(&shared, RunMode::StepOut(depth));
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "pause" => {
                shared.inner.lock().unwrap().pause_requested = true;
                writer.lock().unwrap().respond(&msg, Value::Null)?;
            }
            "disconnect" | "terminate" => {
                writer.lock().unwrap().respond(&msg, Value::Null)?;
                // Release any paused hook so the wasm thread can unwind and exit.
                resume(&shared, RunMode::Continue);
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

/// Sets the run mode and releases a paused hook (if any).
fn resume(shared: &Arc<Shared>, mode: RunMode) {
    let mut inner = shared.inner.lock().unwrap();
    inner.mode = mode;
    inner.resume = true;
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

fn handle_stack_trace(shared: &Arc<Shared>, sm: &SourceMap) -> Value {
    let inner = shared.inner.lock().unwrap();
    // DAP wants the innermost frame first; our call stack has the outermost first.
    let frames: Vec<Value> = inner
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
                "id": i,
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

/// The `variablesReference` handed out for the innermost frame's Locals scope. Non-top frames use 0
/// (their spilled globals have been overwritten, so no live values are available).
const TOP_LOCALS_REF: i64 = 1000;

fn scopes_body(frame_id: i64) -> Value {
    let reference = if frame_id == 0 { TOP_LOCALS_REF } else { 0 };
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
    if reference != TOP_LOCALS_REF {
        return json!({ "variables": [] });
    }
    let inner = shared.inner.lock().unwrap();
    let variables: Vec<Value> = inner
        .locals
        .iter()
        .map(|v| {
            json!({
                "name": v.name,
                "value": v.value,
                "type": v.type_name,
                "variablesReference": 0,
            })
        })
        .collect();
    json!({ "variables": variables })
}

fn handle_evaluate(msg: &Value, shared: &Arc<Shared>) -> Value {
    let expr = msg
        .pointer("/arguments/expression")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let inner = shared.inner.lock().unwrap();
    match inner.locals.iter().find(|v| v.name == expr) {
        Some(v) => json!({ "result": v.value, "type": v.type_name, "variablesReference": 0 }),
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
) -> JoinHandle<()> {
    std::thread::spawn(move || {
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
    link_debug_hooks(&mut linker, shared, source_map, writer)?;
    linker.define_unknown_imports_as_traps(&module)?;

    let instance = linker.instantiate(&mut store, &module)?;
    if let Ok(main_func) = instance.get_typed_func::<(), ()>(&mut store, crate::mir::abi::ENTRY_FN)
    {
        main_func.call(&mut store, ())?;
    }
    Ok(())
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
/// shared debug state so line hooks can pause execution and snapshot locals.
fn link_debug_hooks(
    linker: &mut Linker<()>,
    shared: &Arc<Shared>,
    source_map: &Arc<SourceMap>,
    writer: &Writer,
) -> Result<()> {
    let sh = shared.clone();
    linker.func_wrap(
        "dream_debug",
        "enter",
        move |_c: Caller<'_, ()>, id: i32| {
            let mut inner = sh.inner.lock().unwrap();
            inner.call_stack.push(FrameState {
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
            inner.call_stack.pop();
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
                if let Some(frame) = inner.call_stack.last_mut() {
                    frame.file = file_id as u32;
                    frame.line = line as u32;
                }
                Shared::should_stop(&mut inner)
            };
            let Some(reason) = stop else {
                return;
            };
            // Snapshot the current frame's locals while we still hold the wasm caller.
            let locals = snapshot_locals(&mut caller, &sh, &sm);
            {
                let mut inner = sh.inner.lock().unwrap();
                inner.locals = locals;
                inner.paused = true;
                inner.resume = false;
            }
            let _ = wr.lock().unwrap().event(
                "stopped",
                json!({
                    "reason": reason.as_str(),
                    "threadId": 1,
                    "allThreadsStopped": true,
                }),
            );
            // Park until the client resumes (continue/step/disconnect).
            let mut inner = sh.inner.lock().unwrap();
            while !inner.resume {
                inner = sh.cv.wait(inner).unwrap();
            }
            inner.paused = false;
        },
    )?;
    Ok(())
}

/// Reads and decodes every named local of the innermost frame from the spill-pool globals.
fn snapshot_locals(
    caller: &mut Caller<'_, ()>,
    shared: &Arc<Shared>,
    sm: &SourceMap,
) -> Vec<VarValue> {
    let func_id = {
        let inner = shared.inner.lock().unwrap();
        inner.call_stack.last().map(|f| f.func_id)
    };
    let Some(func_id) = func_id else {
        return Vec::new();
    };
    let Some(info) = sm.function(func_id) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(info.vars.len());
    for var in &info.vars {
        let raw = read_global_i64(caller, var.global).unwrap_or(0);
        let value = decode_value(caller, raw, var.kind);
        out.push(VarValue {
            name: var.name.clone(),
            value,
            type_name: kind_name(var.kind).to_string(),
        });
    }
    out
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

/// Decodes a spilled 64-bit slot into a human-readable value using the variable's kind.
fn decode_value(caller: &mut Caller<'_, ()>, raw: i64, kind: VarKind) -> String {
    let bits = raw as u64;
    let low = bits as u32;
    match kind {
        VarKind::Int => (low as i32).to_string(),
        VarKind::UInt => low.to_string(),
        VarKind::Byte => (low as u8).to_string(),
        VarKind::Bool => {
            if low != 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        VarKind::Char => char::from_u32(low)
            .map(|c| format!("'{}'", c))
            .unwrap_or_else(|| format!("\\u{{{:x}}}", low)),
        VarKind::Long => raw.to_string(),
        VarKind::ULong => bits.to_string(),
        VarKind::Float => f32::from_bits(low).to_string(),
        VarKind::Double => f64::from_bits(bits).to_string(),
        VarKind::Str => {
            let ptr = low as i32;
            if ptr == 0 {
                "null".to_string()
            } else {
                match caller.get_export("memory").and_then(Extern::into_memory) {
                    Some(mem) => format!("\"{}\"", read_string_from_memory(&mem, &*caller, ptr)),
                    None => format!("0x{:x}", low),
                }
            }
        }
        VarKind::Ref => {
            if low == 0 {
                "null".to_string()
            } else {
                format!("0x{:x}", low)
            }
        }
    }
}

fn kind_name(kind: VarKind) -> &'static str {
    match kind {
        VarKind::Int => "int",
        VarKind::UInt => "uint",
        VarKind::Byte => "byte",
        VarKind::Bool => "bool",
        VarKind::Char => "char",
        VarKind::Long => "long",
        VarKind::ULong => "ulong",
        VarKind::Float => "float",
        VarKind::Double => "double",
        VarKind::Str => "string",
        VarKind::Ref => "ref",
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
