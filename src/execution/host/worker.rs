//! Real parallel `WebWorker` host functions (the `Dream` module behind
//! `src/stdlib/core/webworker.dream`).
//!
//! Each worker runs on its own OS thread with a fresh wasmtime `Store` + `Instance` of the *same*
//! module, so it has a completely separate linear memory - nothing is shared. Messages cross the
//! boundary as copied UTF-8 strings over `std::sync::mpsc` channels. The worker thread drives the
//! message loop from Rust: for each inbound message it writes the string into the worker instance's
//! memory and calls the exported `__dream_worker_invoke` trampoline (one `call_indirect` on the
//! `fun(string): string` body), then ships the reply string back.
//!
//! The owner side mirrors the async-future bridge used by `http.rs`: `workerRecv` blocks on the
//! reply channel and pre-resolves a host `Future`, so `await w.receive()` works under wasmtime
//! exactly as it does in the browser (where the reply arrives via `onmessage`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use wasmtime::*;

use crate::mir::abi;
use crate::mir::async_emit::{F_SLOTS, HOST_POLL_INDEX, KIND_HOST};

use super::memory::{read_arg_string, read_string_from_memory, write_string_to_memory};

const TAG_STRING: i32 = abi::TAG_STRING;
const LEN_PREFIX: i32 = abi::LEN_PREFIX_SIZE as i32;

/// A unit of work sent from the owner to a worker thread.
enum Job {
    /// Run the body on this message.
    Message(String),
    /// Shut the worker thread down.
    Terminate,
}

/// Owner-side channel ends for one live worker.
struct WorkerHandle {
    to_worker: Sender<Job>,
    /// Wrapped so `workerRecv` can block on the reply without holding the registry lock (keeping
    /// different workers' receives independent, hence genuinely parallel).
    from_worker: Arc<Mutex<Receiver<String>>>,
}

/// Process-wide registry of live workers, keyed by a globally unique id.
fn workers() -> &'static Mutex<HashMap<u32, WorkerHandle>> {
    static WORKERS: OnceLock<Mutex<HashMap<u32, WorkerHandle>>> = OnceLock::new();
    WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

thread_local! {
    /// The module bytes a worker should instantiate, set per host thread. A thread-local (not a
    /// global) so parallel test suites compiling different variants of the same program never race
    /// on module identity. `set_worker_module` is called on the main/host thread before running,
    /// and re-established on each worker thread so nested spawns work.
    static WASM_BYTES: std::cell::RefCell<Option<Arc<Vec<u8>>>> = const { std::cell::RefCell::new(None) };
}

/// Records the module bytes the current host thread's workers should instantiate. Call once before
/// running a module that may spawn workers (`execute_wasm` and the E2E harness both do).
pub fn set_worker_module(bytes: &[u8]) {
    let arc = Arc::new(bytes.to_vec());
    WASM_BYTES.with(|c| *c.borrow_mut() = Some(arc));
}

fn module_bytes() -> Option<Arc<Vec<u8>>> {
    WASM_BYTES.with(|c| c.borrow().clone())
}

/// Calls an exported `(i32, i32) -> ()` function on the caller module by name (used for
/// `__dream_resolve`). Missing/mistyped export becomes a trap rather than a host abort.
fn call_export_2(caller: &mut Caller<'_, ()>, name: &str, a: i32, b: i32) -> Result<()> {
    let func = caller
        .get_export(name)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg(format!("module must export `{}`", name)))?
        .typed::<(i32, i32), ()>(&*caller)
        .map_err(|_| Error::msg(format!("unexpected `{}` signature", name)))?;
    func.call(&mut *caller, (a, b))?;
    Ok(())
}

/// Bridges a ready reply `String` into the owner's async runtime: allocate a host `Future`, write
/// the string into the owner's memory, resolve the future, and return the future pointer (already
/// settled, so the awaiting task resumes on the next poll). Mirrors `http::resolve_host_future_bytes`.
fn resolve_host_future_string(caller: &mut Caller<'_, ()>, s: &str) -> Result<i32> {
    let new_future = caller
        .get_export(abi::EXPORT_NEW_FUTURE)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg("module must export `__dream_new_future`"))?
        .typed::<(i32, i32, i32), i32>(&*caller)
        .map_err(|_| Error::msg("unexpected `__dream_new_future` signature"))?;
    let future = new_future.call(&mut *caller, (F_SLOTS, HOST_POLL_INDEX, KIND_HOST))?;
    let data_ptr = write_string_to_memory(caller, s)?;
    call_export_2(caller, abi::EXPORT_RESOLVE, future, data_ptr)?;
    Ok(future)
}

/// Writes `s` as a Dream `string` into a worker instance's memory via its exported `malloc`,
/// returning the data pointer. The `Store`-based analogue of `memory::write_string_to_memory`
/// (which needs a `Caller`), used from the worker thread that owns the `Store` directly.
fn store_write_string(
    store: &mut Store<()>,
    malloc: &TypedFunc<(i32, i32), i32>,
    memory: &Memory,
    s: &str,
) -> Option<i32> {
    let bytes = s.as_bytes();
    let ptr = malloc
        .call(&mut *store, (LEN_PREFIX + bytes.len() as i32, TAG_STRING))
        .ok()?;
    let base = ptr as usize;
    let data = memory.data_mut(&mut *store);
    if base + LEN_PREFIX as usize + bytes.len() > data.len() {
        return None;
    }
    data[base..base + LEN_PREFIX as usize].copy_from_slice(&(bytes.len() as i32).to_le_bytes());
    data[base + LEN_PREFIX as usize..base + LEN_PREFIX as usize + bytes.len()]
        .copy_from_slice(bytes);
    Some(ptr)
}

/// The worker thread body: instantiate a fresh copy of the module and run the message loop, calling
/// the `fun(string): string` body (funcref `fn_idx`) once per inbound message. Exits (dropping
/// `reply_tx`, which unblocks any pending owner `recv`) on `Terminate`, channel close, or any
/// instantiation failure.
fn worker_thread(bytes: Arc<Vec<u8>>, fn_idx: i32, job_rx: Receiver<Job>, reply_tx: Sender<String>) {
    // Re-establish the module bytes on this thread so a worker can itself spawn sub-workers.
    WASM_BYTES.with(|c| *c.borrow_mut() = Some(bytes.clone()));

    let engine = Engine::default();
    let Ok(module) = Module::new(&engine, &bytes[..]) else {
        return;
    };
    let mut store = Store::new(&engine, ());
    let mut linker: Linker<()> = Linker::new(&engine);
    build_worker_linker(&mut linker);
    if linker.define_unknown_imports_as_traps(&module).is_err() {
        return;
    }
    let Ok(instance) = linker.instantiate(&mut store, &module) else {
        return;
    };
    let Ok(invoke) =
        instance.get_typed_func::<(i32, i32), i32>(&mut store, abi::EXPORT_WORKER_INVOKE)
    else {
        return;
    };
    let Ok(malloc) = instance.get_typed_func::<(i32, i32), i32>(&mut store, abi::EXPORT_MALLOC)
    else {
        return;
    };
    let Some(memory) = instance
        .get_export(&mut store, abi::EXPORT_MEMORY)
        .and_then(Extern::into_memory)
    else {
        return;
    };

    while let Ok(job) = job_rx.recv() {
        match job {
            Job::Terminate => break,
            Job::Message(msg) => {
                let reply = match store_write_string(&mut store, &malloc, &memory, &msg) {
                    Some(ptr) => match invoke.call(&mut store, (fn_idx, ptr)) {
                        Ok(reply_ptr) => read_string_from_memory(&memory, &store, reply_ptr),
                        Err(_) => String::new(),
                    },
                    None => String::new(),
                };
                if reply_tx.send(reply).is_err() {
                    break; // owner gone
                }
            }
        }
    }
}

/// Minimal host imports for a worker instance: printing to real stdout plus the worker functions
/// themselves (so a worker can spawn sub-workers). Everything else is stubbed as a trap by the
/// caller via `define_unknown_imports_as_traps`, so compute-only workers instantiate cleanly.
fn build_worker_linker(linker: &mut Linker<()>) {
    let _ = linker.func_wrap("env", "print_int", |v: i32| print!("{}", v));
    let _ = linker.func_wrap("env", "print_float", |v: f32| print!("{}", v));
    let _ = linker.func_wrap("env", "print_double", |v: f64| print!("{}", v));
    let _ = linker.func_wrap("env", "print_char", |v: i32| {
        if let Some(c) = char::from_u32(v as u32) {
            print!("{}", c);
        }
    });
    let _ = linker.func_wrap(
        "env",
        "print_string",
        |mut caller: Caller<'_, ()>, ptr: i32| -> Result<()> {
            let memory = caller
                .get_export(abi::EXPORT_MEMORY)
                .and_then(Extern::into_memory)
                .ok_or_else(|| Error::msg("module must export `memory`"))?;
            print!("{}", read_string_from_memory(&memory, &caller, ptr));
            Ok(())
        },
    );
    let _ = link_worker_functions(linker);
}

/// Registers the `WebWorker` host functions on `linker` (the owner side). Safe to call on both the
/// top-level runner's linker and each worker instance's linker (for nested spawns).
pub fn link_worker_functions(linker: &mut Linker<()>) -> Result<()> {
    // workerSpawn(body_funcref) -> id: start a thread running a fresh instance of the module.
    linker.func_wrap(
        "Dream",
        "workerSpawn",
        |_caller: Caller<'_, ()>, fn_idx: i32| -> Result<i32> {
            let bytes =
                module_bytes().ok_or_else(|| Error::msg("worker module bytes not initialized"))?;
            let (job_tx, job_rx) = channel::<Job>();
            let (reply_tx, reply_rx) = channel::<String>();
            std::thread::spawn(move || worker_thread(bytes, fn_idx, job_rx, reply_tx));
            let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
            workers().lock().unwrap().insert(
                id,
                WorkerHandle {
                    to_worker: job_tx,
                    from_worker: Arc::new(Mutex::new(reply_rx)),
                },
            );
            Ok(id as i32)
        },
    )?;

    // workerPost(id, msg): enqueue a message to the worker's inbox (non-blocking).
    linker.func_wrap(
        "Dream",
        "workerPost",
        |mut caller: Caller<'_, ()>, id: i32, msg_ptr: i32| -> Result<()> {
            let msg = read_arg_string(&mut caller, msg_ptr)?;
            let sender = workers()
                .lock()
                .unwrap()
                .get(&(id as u32))
                .map(|h| h.to_worker.clone());
            if let Some(tx) = sender {
                let _ = tx.send(Job::Message(msg));
            }
            Ok(())
        },
    )?;

    // workerRecv(id) -> future: block for the next reply, pre-resolve a host future with it.
    linker.func_wrap(
        "Dream",
        "workerRecv",
        |mut caller: Caller<'_, ()>, id: i32| -> Result<i32> {
            let receiver = workers()
                .lock()
                .unwrap()
                .get(&(id as u32))
                .map(|h| h.from_worker.clone());
            let reply = match receiver {
                Some(rx) => {
                    let guard = rx.lock().unwrap();
                    guard.recv().unwrap_or_default()
                }
                None => String::new(),
            };
            resolve_host_future_string(&mut caller, &reply)
        },
    )?;

    // workerTerminate(id): stop the worker and drop its registration (idempotent).
    linker.func_wrap(
        "Dream",
        "workerTerminate",
        |_caller: Caller<'_, ()>, id: i32| {
            if let Some(handle) = workers().lock().unwrap().remove(&(id as u32)) {
                let _ = handle.to_worker.send(Job::Terminate);
            }
        },
    )?;

    Ok(())
}
