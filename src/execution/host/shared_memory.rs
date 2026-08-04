//! Shared WASM linear memory wiring (WASM threads proposal), used by every wasmtime execution entry
//! point (`execution::wasm_runner`, `execution::debugger::runner`, `execution::host::worker`, and the
//! test harnesses) so the owner instance and every spawned `WebWorker` instance of the *same* module
//! import one identical `wasmtime::SharedMemory` rather than each getting a private linear memory.
//!
//! `src/mir/emit/module.rs` emits linear memory as an *import* (`(import "env" "memory" (memory min
//! max shared))`) rather than a module-defined memory specifically so this crossing is possible: a
//! `wasmtime::SharedMemory` can only be handed to multiple `Instance`s if the module imports its
//! memory instead of instantiating its own.

use wasmtime::*;

/// A `wasmtime::Config` with the WASM threads proposal + `SharedMemory` creation enabled, plus the
/// stack-size tuning every execution entry point already needs (a recursive ARC release chains one
/// wasm frame per node, undersizing the default 512 KiB stack for ordinary-sized data structures).
pub fn threaded_wasm_config() -> Config {
    let mut config = Config::new();
    config.max_wasm_stack(16 * 1024 * 1024);
    config.async_stack_size(20 * 1024 * 1024);
    config.wasm_threads(true);
    config.shared_memory(true);
    // `WebWorker::spawn_worker_thread` compiles a fresh `Module` for every worker from a plain
    // `std::thread`, not a rayon worker. Wasmtime's default parallel compilation submits its
    // codegen work to rayon's *global* thread pool — the same pool a caller may already be using
    // for its own outer parallelism (e.g. this test suite's `tests/e2e_tests.rs` corpus runner).
    // If every thread in that global pool is already blocked waiting on a `rayon::join` from the
    // outer parallelism, the worker thread's `Module::new` call has nowhere to run its compilation
    // task and the whole process deadlocks. Compiling serially per worker avoids the reentrancy
    // hazard entirely; a single small module compiles fast enough that this costs nothing visible.
    config.parallel_compilation(false);
    config
}

/// Allocates a `SharedMemory` matching the `"env"."memory"` import type declared by `module` — the
/// single backing store every instance of this module (the owner instance and every worker thread
/// spawned afterward) imports, so linear memory is genuinely shared rather than copied.
pub fn shared_memory_for(engine: &Engine, module: &Module) -> Result<SharedMemory> {
    for import in module.imports() {
        if import.module() == "env" && import.name() == "memory" {
            if let ExternType::Memory(mt) = import.ty() {
                let min = mt.minimum() as u32;
                let max = mt.maximum().map(|m| m as u32).unwrap_or(min);
                return SharedMemory::new(engine, MemoryType::shared(min, max));
            }
        }
    }
    Err(Error::msg(
        "module does not import `env.memory` as a shared memory",
    ))
}
