use super::*;

/// The emission-driving lookup tables derived once per module and shared by every per-function emit:
/// resolved call symbols, callee signatures (for argument widening), the interned-string address map,
/// struct/union type tags, the indirect-call function table, and the set of value structs needing
/// retain/drop glue. Built by [`build_tables`] so [`emit_program`] and [`emit_module_with_debug`]
/// derive them identically.
struct ModuleTables {
    symbols: HashMap<(DefId, Vec<TypeId>), String>,
    sigs: HashMap<(DefId, Vec<TypeId>), Vec<TypeId>>,
    strings: IndexMap<String, u32>,
    tags: HashMap<TypeId, i32>,
    ftable: HashMap<(DefId, Vec<TypeId>), usize>,
    value_glue: std::collections::HashSet<TypeId>,
}

/// Derives the shared [`ModuleTables`] for `mir`.
///
/// `locate_panics` controls whether runtime check messages include file:line (debug / debug-info)
/// or share four compact base strings (release).
fn build_tables(mir: &crate::mir::Mir, interner: &TypeInterner, locate_panics: bool) -> ModuleTables {
    ModuleTables {
        symbols: symbol_table(mir),
        sigs: signature_table(mir),
        strings: string_table(mir, interner, locate_panics),
        tags: struct_tags(mir),
        ftable: func_table(mir),
        value_glue: value_glue_types(mir, interner),
    }
}

/// Emits a whole MIR program as a sequence of WAT function definitions (no module wrapper). Used by
/// the pipeline tests; the driver target is [`emit_module`].
pub fn emit_program(mir: &crate::mir::Mir, interner: &TypeInterner) -> String {
    let ModuleTables {
        symbols,
        sigs,
        strings,
        tags,
        ftable,
        value_glue,
    } = build_tables(mir, interner, true);
    let global_tys: HashMap<u32, TypeId> = mir.globals.iter().map(|g| (g.id.0, g.ty)).collect();
    let mut out = String::new();
    for f in &mir.functions {
        out.push_str(&emit_function_with(
            f,
            interner,
            &symbols,
            &sigs,
            &mir.layouts,
            &strings,
            &tags,
            &ftable,
            &value_glue,
            &global_tys,
            false,
            true,
            None,
        ));
        out.push('\n');
    }
    out
}

/// Emits a whole MIR program as a single `(module ...)`, exporting every (non-instance) function
/// under its source name. This is the self-contained unit the driver hands to the WASM assembler.
pub fn emit_module(mir: &crate::mir::Mir, interner: &TypeInterner, debug: bool) -> String {
    emit_module_with_debug(mir, interner, debug, false).0
}

/// Like [`emit_module`], but when `debug_info` is set it also instruments every function with the
/// `dream_debug` source-line hooks + local spilling and returns the [`DebugModule`] source map
/// describing them. When `debug_info` is false the returned map is `None` and the WAT is identical
/// to [`emit_module`].
pub fn emit_module_with_debug(
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    debug: bool,
    debug_info: bool,
) -> (String, Option<crate::mir::emit::debug_map::DebugModule>) {
    // Located (file:line) panic strings are useful while debugging; release builds (`!debug &&
    // !debug_info`) share four compact base messages instead to keep the data section small.
    let locate_panics = debug || debug_info;
    let ModuleTables {
        symbols,
        sigs,
        strings,
        tags,
        ftable,
        value_glue,
    } = build_tables(mir, interner, locate_panics);
    let global_tys: HashMap<u32, TypeId> = mir.globals.iter().map(|g| (g.id.0, g.ty)).collect();

    // Debug-info metadata (file table + per-function variable tables + spill-pool width). Built up
    // front so both the instrumentation below and the returned source map agree on ids/slots.
    let dbg_module = if debug_info {
        Some(crate::mir::emit::debug_map::DebugModule::build(
            mir, interner, &symbols,
        ))
    } else {
        None
    };
    // Symbol -> its debug metadata, so the per-function emit can hand the emitter its var table.
    let dbg_by_symbol: HashMap<&str, &crate::mir::emit::debug_map::DebugFunction> = dbg_module
        .as_ref()
        .map(|m| m.functions.iter().map(|f| (f.symbol.as_str(), f)).collect())
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("(module\n");

    // Imports come first (WASM requires imported funcs before defined ones).
    emit_imports(&mut out, mir, interner);
    // Debug hook imports (only referenced by instrumented functions).
    if debug_info {
        let m = crate::mir::emit::debug_map::DEBUG_MODULE;
        let _ = writeln!(
            out,
            "(import \"{m}\" \"line\" (func $__dbg_line (param i32) (param i32)))"
        );
        let _ = writeln!(
            out,
            "(import \"{m}\" \"enter\" (func $__dbg_enter (param i32)))"
        );
        let _ = writeln!(
            out,
            "(import \"{m}\" \"exit\" (func $__dbg_exit (param i32)))"
        );
    }

    // The shared-memory import (below) must textually precede every non-import module field (WASM's
    // fixed section order puts the whole import section first), but its page count is only known
    // once the static-data/interface-dispatch layout below has been computed. Remember this
    // position and splice the import in once that size is known, rather than reordering the many
    // interdependent emission steps that follow.
    let memory_import_pos = out.len();

    // `call_indirect` signature types (declared before use), plus the function table + its export.
    emit_func_signatures(&mut out, interner);
    emit_func_table(&mut out, mir);

    // Interface dispatch tables live in linear memory just past the interned strings and any
    // value-struct global BSS; the heap bump pointer then starts past those.
    let used_slots = used_iface_slots(mir);
    let (vg_addrs, static_end) = value_global_addrs(mir, interner, heap_base(&strings));
    let iface = emit_interface_dispatch(mir, interner, static_end, &used_slots);

    // Linear memory + allocator runtime state. Layout (low -> high): static data (strings +
    // value-struct global BSS + itables) | shadow-stack region (grows down) | heap (grows up).
    // The shadow stack and heap grow away from a shared boundary in opposite directions, so they
    // never collide. `iface.heap_start` is the end of the static data; the shadow stack occupies
    // the next SHADOW_STACK_SIZE bytes and the heap begins at the top of that region.
    let data_end = iface.heap_start;
    let heap_base = data_end + SHADOW_STACK_SIZE;
    let initial_pages = heap_base.div_ceil(WASM_PAGE_SIZE) + INITIAL_HEAP_PAGES;
    // Imported (not module-defined) and `shared`: a `wasmtime::SharedMemory`/JS `WebAssembly.Memory
    // ({shared:true})` created by the host is handed to the owner instance and every subsequently
    // spawned `WebWorker` instance of this same module, so linear memory is genuinely shared across
    // threads rather than copied. The threads proposal requires a shared memory to declare a fixed
    // maximum up front; `MAX_MEMORY_PAGES` is the wasm32 address-space ceiling, not a real cap on
    // heap growth (the allocator's `memory.grow` bump path is otherwise unbounded). Spliced in at
    // `memory_import_pos` (see above) so it textually precedes the func table/signatures already
    // emitted, satisfying WASM's import-section-comes-first ordering.
    out.insert_str(
        memory_import_pos,
        &format!(
            "(import \"env\" \"memory\" (memory {} {} shared))\n",
            initial_pages,
            crate::mir::abi::MAX_MEMORY_PAGES
        ),
    );
    out.push_str("(global $free_list_head (mut i32) (i32.const 0))\n");
    // Per-instance (per-thread) cache of this thread's id — see `runtime/sync.wat`'s `$__thread_id`
    // and `THREAD_ID_COUNTER_ADDR`'s doc comment in `src/mir/abi.rs`.
    out.push_str("(global $__tid (mut i32) (i32.const 0))\n");
    // Shadow-stack pointer for inline value (`struct`) locals; grows down from the heap base toward
    // the static data (its region floor).
    let _ = writeln!(out, "(global $__sp (mut i32) (i32.const {}))", heap_base);
    out.push_str("(global $live_objects (mut i32) (i32.const 0))\n");
    out.push_str("(global $total_allocations (mut i32) (i32.const 0))\n");

    // Module-level user variables. Scalars/references start zeroed; value-struct globals hold the
    // address of their permanent BSS slot (constructed in `$__dream_init`).
    for g in &mir.globals {
        if let Some(&addr) = vg_addrs.get(&g.id.0) {
            let _ = writeln!(
                out,
                "(global $g{} (mut i32) (i32.const {}))",
                g.id.0, addr
            );
        } else {
            let zero = zero_literal(wasm_ty_of(interner, g.ty));
            let _ = writeln!(
                out,
                "(global $g{} (mut {}) {})",
                g.id.0,
                wasm_ty_of(interner, g.ty),
                zero
            );
        }
    }

    // Debug-info spill pool: one exported mutable `i64` global per live-local slot. Each named local
    // is spilled here at every statement boundary so the debugger can read live values from the host.
    if let Some(m) = &dbg_module {
        for k in 0..m.global_pool {
            let _ = writeln!(out, "(global $__dbg_v{k} (mut i64) (i64.const 0))");
            let _ = writeln!(out, "(export \"__dbg_v{k}\" (global $__dbg_v{k}))");
        }
    }

    out.push_str(&runtime_prelude(debug));
    out.push('\n');
    out.push_str(RUNTIME_WEAK);
    out.push('\n');
    out.push_str(RUNTIME_CLOSURE);
    out.push('\n');
    out.push_str(&RUNTIME_SYNC.replace(
        "{THREAD_ID_COUNTER_ADDR}",
        &crate::mir::abi::THREAD_ID_COUNTER_ADDR.to_string(),
    ));
    out.push('\n');
    if crate::mir::async_emit::module_has_async(&mir.functions) {
        out.push_str(&crate::mir::async_emit::async_runtime_wat());
        out.push('\n');
    }
    out.push_str(&to_string_runtime(&strings));
    out.push('\n');
    out.push_str(RUNTIME_PANIC);
    out.push('\n');
    emit_object_protocol(&mut out, mir, interner, &strings, &tags);
    out.push('\n');
    emit_js_marshal(&mut out, mir, interner, &strings, &tags);
    out.push('\n');
    emit_release_funcs(&mut out, mir, interner, &tags, &value_glue);
    out.push('\n');
    emit_value_glue(&mut out, mir, interner, &value_glue);
    out.push('\n');

    // Interface dispatch trampolines (reference `$object_tag` + `$__ft`, both defined above).
    out.push_str(&iface.trampolines);
    if !iface.trampolines.is_empty() {
        out.push('\n');
    }

    for (s, addr) in &strings {
        // The data segment is the full heap block, written at the block start (header before data).
        let block = addr - HEAP_HEADER_SIZE;
        let _ = writeln!(out, "(data (i32.const {}) \"{}\")", block, escape_data(s));
    }

    // Interface itable data segments (tag-indexed method tables), past the string region.
    out.push_str(&iface.data);

    let polls = crate::mir::async_emit::poll_indices(&mir.functions);
    let mut has_init = false;
    for f in &mir.functions {
        if f.is_async {
            let debug_fn = dbg_by_symbol.get(func_symbol(f).as_str()).copied();
            out.push_str(&crate::mir::async_emit::emit_async_function(
                f,
                interner,
                &symbols,
                &mir.layouts,
                &strings,
                &tags,
                &ftable,
                *polls.get(&(f.def, f.instance.clone())).unwrap_or(&0),
                debug,
                locate_panics,
                debug_fn,
            ));
        } else {
            let debug_fn = dbg_by_symbol.get(func_symbol(f).as_str()).copied();
            out.push_str(&emit_function_with(
                f,
                interner,
                &symbols,
                &sigs,
                &mir.layouts,
                &strings,
                &tags,
                &ftable,
                &value_glue,
                &global_tys,
                debug,
                locate_panics,
                debug_fn,
            ));
        }
        if f.name == crate::mir::lower::INIT_FN_NAME {
            has_init = true;
        } else if f.instance.is_empty() && f.name == crate::mir::abi::ENTRY_FN && f.is_async {
            out.push_str(&crate::mir::async_emit::emit_async_main_wrapper(
                &func_symbol(f),
                !f.params.is_empty(),
            ));
        } else if f.instance.is_empty()
            && f.name == crate::mir::abi::ENTRY_FN
            && !f.params.is_empty()
        {
            // `main(args: string[])`: the exported entry takes no args, so wrap the real `main` with a
            // `()` shim that passes an empty `string[]` (a zero-length, TAG_ARRAY block).
            let _ = writeln!(
                out,
                "(func (export \"main\")\n (local $args i32)\n i32.const 4\n i32.const {}\n call $malloc\n local.set $args\n local.get $args\n i32.const 0\n i32.store\n local.get $args\n call ${}\n)",
                crate::mir::abi::TAG_ARRAY,
                func_symbol(f),
            );
        } else if f.instance.is_empty() {
            let _ = writeln!(out, "(export \"{}\" (func ${}))", f.name, func_symbol(f));
        }
        out.push('\n');
    }

    // Worker-thread trampoline: given a `fun(string): string` body's funcref index, its closure
    // environment word (0 for a non-capturing body; an `@shared`-object pointer or a by-value
    // `CaptureCell`/`object[]` env for a capturing one — see `analyze_lambda`'s `WebWorker` capture
    // check), and a message string pointer, publish the env to `$g0` (the synthetic
    // `__closure_env` global every module registers first — see `register_globals` — so a
    // capturing callee's own prologue reads the right environment on whichever thread invokes it)
    // then perform one indirect call and return the reply pointer. Emitted for every module (it
    // only depends on the always-present `$__ft` table and `$g0`) so a freshly instantiated worker
    // of the same module can be driven from the host (see `src/stdlib/core/webworker.dream`).
    //
    // A body's funcref index may name an `async fun`'s *constructor* rather than an ordinary
    // function (the analyzer allows this specifically for a `WebWorker`/`.map`/`.dispatch` body
    // argument — see `Analyzer::is_webworker_body_call`); calling it synchronously here would hand
    // back a raw `Future` frame pointer where the caller expects the real reply value. Every heap
    // allocation carries a tag except a `Future` frame (`dream_new_future` mallocs with tag `0`,
    // a value never used by any real Dream type — see `abi::TAG_*`), so that is the exact, cheap
    // signal to distinguish the two cases: an untagged, non-null result means the call_indirect hit
    // an async constructor, so drive it to completion (`$dream_run_loop` — sound only because every
    // native host `async` op resolves synchronously before returning, so one drain pass always
    // finishes the task; see `docs/language/webworkers.md`'s async-body section for why this does
    // *not* hold for the browser `Worker` backend) and unwrap its settled `Future.result` in place
    // of the constructor's own return value. An ordinary (non-async) body's result is already
    // tagged (`string`, at minimum) and passes straight through untouched.
    out.push_str("(type $__worker_sig (func (param i32) (result i32)))\n");
    // `$g0` (the synthetic `__closure_env` global — see `register_globals`) only exists in modules
    // that went through the full front-end; a handful of backend unit tests assemble a minimal
    // `Mir` directly (skipping `register_globals` entirely) and have no globals at all, so guard
    // the env-publishing write on it actually being present rather than assuming it unconditionally.
    let has_closure_env = mir.globals.iter().any(|g| g.id.0 == 0);
    // `$dream_run_loop` (and hence a `Future`'s `F_RESULT` slot) only exists once the async runtime
    // is spliced in (see above) — a module with no `async fun` at all (so no `WebWorker` body could
    // possibly be one) never needs the drive-to-completion check.
    let has_async_runtime = crate::mir::async_emit::module_has_async(&mir.functions);
    let publish_env = if has_closure_env {
        " local.get $env\n global.set $g0\n"
    } else {
        ""
    };
    // The raw call: publish the closure env (if any) then one `call_indirect`. Exported as-is for
    // the browser driver (see `EXPORT_WORKER_INVOKE_RAW`'s doc comment); wrapped below with the
    // drive-to-completion check for the native driver.
    let _ = writeln!(
        out,
        "(func $__dream_worker_invoke_raw (param $fn i32) (param $env i32) (param $arg i32) (result i32)\n{publish_env} local.get $arg\n local.get $fn\n call_indirect $__ft (type $__worker_sig))"
    );
    if has_async_runtime {
        let _ = writeln!(
            out,
            "(func $__dream_worker_invoke (param $fn i32) (param $env i32) (param $arg i32) (result i32)\n (local $r i32)\n local.get $fn\n local.get $env\n local.get $arg\n call $__dream_worker_invoke_raw\n local.set $r\n local.get $r\n i32.const 0\n i32.ne\n local.get $r\n call $object_tag\n i32.eqz\n i32.and\n (if\n  (then\n   call $dream_run_loop\n   local.get $r\n   i32.load offset={}\n   local.set $r\n  )\n )\n local.get $r)",
            crate::mir::async_emit::F_RESULT,
        );
    } else {
        let _ = writeln!(
            out,
            "(func $__dream_worker_invoke (param $fn i32) (param $env i32) (param $arg i32) (result i32)\n local.get $fn\n local.get $env\n local.get $arg\n call $__dream_worker_invoke_raw)"
        );
    }

    // Every instantiation of this module (the owner, and every `WebWorker` spawned afterward) runs
    // `(start)` against the *same* shared linear memory. The bump-pointer heap high-water mark
    // (`HEAP_PTR_ADDR`) must therefore be initialized exactly once across all of them, not reset to
    // `heap_base` on every instantiation (that would let a later instance re-bump-allocate over an
    // earlier instance's live objects). An atomic compare-exchange from 0 makes this init race-safe:
    // whichever instance's `$__runtime_init` runs first wins the exchange, every later one is a no-op.
    let _ = writeln!(out, "(func $__runtime_init");
    let _ = writeln!(
        out,
        "  i32.const {}\n  i32.const 0\n  i32.const {}\n  i32.atomic.rmw.cmpxchg\n  drop",
        crate::mir::abi::HEAP_PTR_ADDR,
        heap_base
    );
    if has_init {
        let _ = writeln!(out, "  call ${}", crate::mir::lower::INIT_FN_NAME);
    }
    out.push_str(")\n");
    out.push_str("(start $__runtime_init)\n");

    // Host-facing exports: memory and the allocator (so a JS runtime can build heap values).
    use crate::mir::abi;
    let _ = writeln!(out, "(export \"{}\" (memory 0))", abi::EXPORT_MEMORY);
    let _ = writeln!(out, "(export \"{}\" (func $malloc))", abi::EXPORT_MALLOC);
    let _ = writeln!(out, "(export \"{}\" (func $free))", abi::EXPORT_FREE);
    let _ = writeln!(
        out,
        "(export \"{}\" (func $__dream_worker_invoke))",
        abi::EXPORT_WORKER_INVOKE
    );
    let _ = writeln!(
        out,
        "(export \"{}\" (func $__dream_worker_invoke_raw))",
        abi::EXPORT_WORKER_INVOKE_RAW
    );
    if crate::mir::async_emit::module_has_async(&mir.functions) {
        let _ = writeln!(
            out,
            "(export \"{}\" (func $dream_run_loop))",
            abi::EXPORT_RUN_LOOP
        );
        let _ = writeln!(
            out,
            "(export \"{}\" (func $dream_resolve))",
            abi::EXPORT_RESOLVE
        );
        let _ = writeln!(
            out,
            "(export \"{}\" (func $dream_new_future))",
            abi::EXPORT_NEW_FUTURE
        );
    }
    out.push_str(")\n");
    // Whole-module dead-function elimination: drop embedded runtime helpers (and any other funcs)
    // not reachable from the module's exports / start / function table. Runs under `--release`
    // (and any other uninstrumented build); skipped in debug and debug-info builds (which keep the
    // full module for inspection/debugging).
    let wat = if !debug && !debug_info {
        strip_dead_functions(&out)
    } else {
        out
    };
    (wat, dbg_module)
}

/// Emits the module's `(import ...)` declarations: the fixed host `print_*` builtins (which
/// `print`/`println` lower to) followed by user `extern fun` interop imports. Call sites reference
/// each import's internal `$name`; the `module`/`field` pair names the host binding.
pub(super) fn emit_imports(out: &mut String, mir: &crate::mir::Mir, interner: &TypeInterner) {
    for (name, param) in [
        ("print_string", "i32"),
        ("print_int", "i32"),
        ("print_float", "f32"),
        ("print_double", "f64"),
        ("print_char", "i32"),
    ] {
        let _ = writeln!(
            out,
            "(import \"{}\" \"{name}\" (func ${name} (param {param})))",
            crate::mir::abi::ENV_MODULE
        );
    }
    for imp in &mir.imports {
        let params: String = imp
            .params
            .iter()
            .map(|t| format!(" {}", wasm_ty_of(interner, *t)))
            .collect();
        let params = if params.is_empty() {
            String::new()
        } else {
            format!(" (param{params})")
        };
        let result = match imp.ret {
            Some(t) => format!(" (result {})", wasm_ty_of(interner, t)),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "(import \"{}\" \"{}\" (func ${}{}{}))",
            imp.module, imp.field, imp.name, params, result
        );
    }
}
