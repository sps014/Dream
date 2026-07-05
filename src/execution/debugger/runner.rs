//! Runs the compiled wasm program under the debug hooks: spawning the execution thread, linking the
//! `dream_debug.*` host hooks and DAP-routed print builtins, and attaching newly spawned `WebWorker`
//! threads as their own DAP threads. Split out of `mod.rs`.

use crate::execution::host::{read_string_from_memory, WorkerDebug};
use crate::execution::wasm_runner::link_runtime_host_functions;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use wasmtime::*;

use super::decode::snapshot_locals;
use super::sourcemap::SourceMap;
use super::state::{FrameState, Inner, Shared, ThreadHot, ThreadState};
use super::{thread_name, Writer, MAIN_THREAD};

/// Spawns the wasm execution thread: builds the engine/linker (with the debug hooks wired to
/// `shared`), instantiates the module, and runs `main`. Sends `exited`/`terminated` DAP events when
/// the program finishes.
pub(super) fn spawn_execution(
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
                t.mode = super::state::RunMode::StepIn;
            }
        }
        if stop_on_entry {
            // Mirror into the lock-free hot state so the very first line hook escalates and stops.
            shared
                .hot_for(MAIN_THREAD)
                .set_mode(super::state::RunMode::StepIn);
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
pub(super) struct WorkerAttach {
    pub(super) shared: Arc<Shared>,
    pub(super) source_map: Arc<SourceMap>,
    pub(super) writer: Writer,
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
    // The lock-free hot state this thread's hooks read on every line without touching the mutex.
    let hot = shared.hot_for(thread_id);

    let sh = shared.clone();
    let hot_enter = hot.clone();
    linker.func_wrap(
        "dream_debug",
        "enter",
        move |_c: Caller<'_, ()>, id: i32| {
            hot_enter.depth.fetch_add(1, Ordering::Relaxed);
            let mut inner = sh.inner.lock().unwrap();
            let t = inner
                .threads
                .entry(thread_id)
                .or_insert_with(|| ThreadState::new(thread_name(thread_id)));
            // Freeze the caller's current line (from the lock-free position) into its frame before
            // pushing the callee — so the caller shows its call site even though most of its lines ran
            // on the mutex-free fast path.
            if let Some(caller_frame) = t.call_stack.last_mut() {
                let packed = hot_enter.pos.load(Ordering::Relaxed);
                caller_frame.file = (packed >> 32) as u32;
                caller_frame.line = packed as u32;
            }
            t.call_stack.push(FrameState {
                func_id: id as u32,
                file: 0,
                line: 0,
            });
        },
    )?;

    let sh = shared.clone();
    let hot_exit = hot.clone();
    linker.func_wrap(
        "dream_debug",
        "exit",
        move |_c: Caller<'_, ()>, _id: i32| {
            hot_exit.depth.fetch_sub(1, Ordering::Relaxed);
            let mut inner = sh.inner.lock().unwrap();
            if let Some(t) = inner.threads.get_mut(&thread_id) {
                t.call_stack.pop();
            }
        },
    )?;

    let sh = shared.clone();
    let sm = source_map.clone();
    let wr = writer.clone();
    let hot_line = hot.clone();
    linker.func_wrap(
        "dream_debug",
        "line",
        move |mut caller: Caller<'_, ()>, file_id: i32, line: i32| {
            // Hot path: record the current position lock-free, then bail unless something might want to
            // stop here (a breakpoint at this line, an active step, or a pending pause). This is what
            // keeps tight loops running at near-native speed under the debugger.
            hot_line.pos.store(
                ThreadHot::pack_pos(file_id as u32, line as u32),
                Ordering::Relaxed,
            );
            let maybe_stop = hot_line.pause.load(Ordering::Relaxed)
                || hot_line.step_wants_stop()
                || sh.bp_filter.probe(file_id as u32, line as u32);
            if !maybe_stop {
                return;
            }

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
            // A stop is happening: clear the lock-free pause flag now that it has been consumed.
            hot_line.pause.store(false, Ordering::Relaxed);
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

pub(super) fn read_caller_string(caller: &mut Caller<'_, ()>, ptr: i32) -> Result<String> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| Error::msg("module must export `memory`"))?;
    Ok(read_string_from_memory(&memory, &*caller, ptr))
}
