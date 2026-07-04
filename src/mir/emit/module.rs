use super::*;

/// Emits a whole MIR program as a sequence of WAT function definitions (no module wrapper). Used by
/// the pipeline tests; the driver target is [`emit_module`].
pub fn emit_program(mir: &crate::mir::Mir, interner: &TypeInterner) -> String {
    let symbols = symbol_table(mir);
    let sigs = signature_table(mir);
    let strings = string_table(mir);
    let tags = struct_tags(mir);
    let ftable = func_table(mir);
    let value_glue = value_glue_types(mir, interner);
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
            false,
        ));
        out.push('\n');
    }
    out
}

/// Emits a whole MIR program as a single `(module ...)`, exporting every (non-instance) function
/// under its source name. This is the self-contained unit the driver will hand to the WASM
/// assembler once the runtime layers are wired in.
pub fn emit_module(mir: &crate::mir::Mir, interner: &TypeInterner, debug: bool) -> String {
    let symbols = symbol_table(mir);
    let sigs = signature_table(mir);
    let strings = string_table(mir);
    let tags = struct_tags(mir);
    let ftable = func_table(mir);
    let value_glue = value_glue_types(mir, interner);
    let mut out = String::new();
    out.push_str("(module\n");

    // Imports come first (WASM requires imported funcs before defined ones).
    emit_imports(&mut out, mir, interner);

    // `call_indirect` signature types (declared before use), plus the function table + its export.
    emit_func_signatures(&mut out, interner);
    emit_func_table(&mut out, mir);

    // Interface dispatch tables live in linear memory just past the interned strings; the heap bump
    // pointer then starts past those. Its trampolines/data are emitted below.
    let used_slots = used_iface_slots(mir);
    let iface = emit_interface_dispatch(mir, interner, heap_base(&strings), &used_slots);

    // Linear memory + allocator runtime state. Layout (low -> high): static data (strings + itables)
    // | shadow-stack region (grows down) | heap (grows up, extends memory via memory.grow). The
    // shadow stack and heap grow away from a shared boundary in opposite directions, so they never
    // collide. `iface.heap_start` is the end of the static data; the shadow stack occupies the next
    // SHADOW_STACK_SIZE bytes and the heap begins at the top of that region.
    let data_end = iface.heap_start;
    let heap_base = data_end + SHADOW_STACK_SIZE;
    let initial_pages = heap_base.div_ceil(WASM_PAGE_SIZE) + INITIAL_HEAP_PAGES;
    let _ = writeln!(out, "(memory {})", initial_pages);
    let _ = writeln!(
        out,
        "(global $heap_ptr (mut i32) (i32.const {}))",
        heap_base
    );
    out.push_str("(global $free_list_head (mut i32) (i32.const 0))\n");
    // Shadow-stack pointer for inline value (`struct`) locals; grows down from the heap base toward
    // the static data (its region floor).
    let _ = writeln!(out, "(global $__sp (mut i32) (i32.const {}))", heap_base);
    out.push_str("(global $live_objects (mut i32) (i32.const 0))\n");
    out.push_str("(global $total_allocations (mut i32) (i32.const 0))\n");

    // Module-level user variables. They start zeroed; any initializer runs in `$__dream_init`
    // (emitted as a normal function below and wired to `(start ...)`).
    for g in &mir.globals {
        let zero = zero_literal(wasm_ty_of(interner, g.ty));
        let _ = writeln!(
            out,
            "(global $g{} (mut {}) {})",
            g.id.0,
            wasm_ty_of(interner, g.ty),
            zero
        );
    }

    out.push_str(&runtime_prelude(debug));
    out.push('\n');
    if crate::mir::async_emit::module_has_async(&mir.functions) {
        out.push_str(&crate::mir::async_emit::async_runtime_wat());
        out.push('\n');
    }
    out.push_str(&to_string_runtime(&strings));
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
            ));
        } else {
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
                debug,
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

    // Run global initializers before any entry point.
    if has_init {
        let _ = writeln!(out, "(start ${})", crate::mir::lower::INIT_FN_NAME);
    }

    // Host-facing exports: memory and the allocator (so a JS runtime can build heap values).
    use crate::mir::abi;
    let _ = writeln!(out, "(export \"{}\" (memory 0))", abi::EXPORT_MEMORY);
    let _ = writeln!(out, "(export \"{}\" (func $malloc))", abi::EXPORT_MALLOC);
    let _ = writeln!(out, "(export \"{}\" (func $free))", abi::EXPORT_FREE);
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
    // not reachable from the module's exports / start / function table.
    if !debug {
        strip_dead_functions(&out)
    } else {
        out
    }
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
