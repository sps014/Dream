use super::*;

/// The emitted symbol for a function (or generic instance): the source name, suffixed with the
/// instance's interned type-arg ids so each monomorphization stays distinct.
pub(crate) fn func_symbol(func: &MirFunction) -> String {
    if func.instance.is_empty() {
        func.name.clone()
    } else {
        let args: Vec<String> = func.instance.iter().map(|t| t.0.to_string()).collect();
        format!("{}__{}", func.name, args.join("_"))
    }
}

/// Maps each function's `(DefId, instance args)` to its emitted symbol, so call sites (which carry
/// the callee's def + monomorphization args) resolve to the same symbol the header uses. Keying by
/// the instance args — not the def alone — keeps distinct generic instances distinct.
pub(super) fn symbol_table(mir: &crate::mir::Mir) -> HashMap<(DefId, Vec<TypeId>), String> {
    let mut table: HashMap<(DefId, Vec<TypeId>), String> = mir
        .functions
        .iter()
        .map(|f| ((f.def, f.instance.clone()), func_symbol(f)))
        .collect();
    // Imports have no MIR body but are call targets: map their def to the imported `$name` so calls
    // resolve to the import instead of the `$def{N}` fallback.
    for imp in &mir.imports {
        table.insert((imp.def, vec![]), imp.name.clone());
    }
    // Intrinsic externs have no body/import: map their def to the intrinsic key so a call resolves to
    // the runtime helper `$<key>` (e.g. `$string_alloc`) or is recognized as an async intrinsic
    // (`sleep`) rather than falling back to `$def{N}`.
    for (def, key) in &mir.intrinsics {
        table.entry((*def, vec![])).or_insert_with(|| key.clone());
    }
    table
}

/// Maps each function's `(DefId, instance args)` to its declared parameter types, so call sites can
/// apply implicit numeric widening (e.g. an `int`/`float` argument passed to a `double` parameter)
/// to match the callee's WASM signature. Keyed like [`symbol_table`].
pub(super) fn signature_table(mir: &crate::mir::Mir) -> HashMap<(DefId, Vec<TypeId>), Vec<TypeId>> {
    mir.functions
        .iter()
        .map(|f| {
            let params = f.params.iter().map(|p| f.local_ty(*p)).collect();
            ((f.def, f.instance.clone()), params)
        })
        .collect()
}

/// Maps each function's `(DefId, instance args)` to its slot in the module's function table, in
/// `mir.functions` order (so the slot index matches the `(elem ...)` position below). A `FuncRef`
/// resolves to this index; `call_indirect` uses it as the table entry.
pub(super) fn func_table(mir: &crate::mir::Mir) -> HashMap<(DefId, Vec<TypeId>), usize> {
    mir.functions
        .iter()
        .enumerate()
        .map(|(i, f)| ((f.def, f.instance.clone()), i))
        .collect()
}

/// The canonical `call_indirect` type name + `(param …)`/`(result …)` WASM types for a function-typed
/// `ty` (nullable stripped). Named by its *WASM* signature (so `fun(int)` and `fun(bool)` share one),
/// which is all `call_indirect` distinguishes. `None` if `ty` is not a function type.
pub(super) fn func_sig(
    interner: &TypeInterner,
    ty: TypeId,
) -> Option<(String, Vec<&'static str>, Option<&'static str>)> {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Func(params, ret) => {
            let mut ptys: Vec<&'static str> =
                params.iter().map(|p| wasm_ty_of(interner, *p)).collect();
            // A value(`struct`/union)-returning function uses the sret ABI: a hidden leading `i32`
            // destination pointer, and no WASM result. `call_indirect` (interface trampolines,
            // first-class function values) must name a `(type ...)` of this exact shape, so model it
            // here — the single source of truth shared by the signature declarations, the trampolines,
            // and the callers.
            if interner.is_value_type(*ret) {
                ptys.insert(0, "i32");
                let name = format!("$sig_sret_{}__v", ptys.join("_"));
                return Some((name, ptys, None));
            }
            let rty = match interner.kind(*ret) {
                TyKind::Void => None,
                _ => Some(wasm_ty_of(interner, *ret)),
            };
            let name = format!("$sig_{}__{}", ptys.join("_"), rty.unwrap_or("v"));
            Some((name, ptys, rty))
        }
        _ => None,
    }
}

/// True when interface method signature `ty` (a `Func(params, ret)`) returns a value type by the
/// sret ABI, so its dispatch trampoline and call sites carry a hidden leading destination pointer.
pub(super) fn func_sig_is_sret(interner: &TypeInterner, ty: TypeId) -> bool {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Func(_, ret) => interner.is_value_type(*ret),
        _ => false,
    }
}

/// Emits a `(type …)` declaration for every distinct function signature in the program (one per WASM
/// shape), so `call_indirect` can name its expected type. Over-approximates from all interned function
/// types — spare declarations are harmless.
pub(super) fn emit_func_signatures(out: &mut String, interner: &TypeInterner) {
    let mut seen: IndexMap<String, (Vec<&'static str>, Option<&'static str>)> = IndexMap::new();
    for (id, kind) in interner.iter_kinds() {
        if matches!(kind, TyKind::Func(..)) {
            if let Some((name, ptys, rty)) = func_sig(interner, id) {
                seen.entry(name).or_insert((ptys, rty));
            }
        }
    }
    for (name, (ptys, rty)) in &seen {
        let params: String = ptys.iter().map(|t| format!(" (param {})", t)).collect();
        let result = rty.map(|t| format!(" (result {})", t)).unwrap_or_default();
        let _ = writeln!(out, "(type {} (func{}{}))", name, params, result);
    }
}

pub(crate) fn poll_symbol(func: &MirFunction) -> String {
    format!("poll_{}", func_symbol(func))
}

/// Emits the function table and its element section (constructors/sync functions first, then async
/// poll functions), plus the `__indirect_function_table` export.
pub(super) fn emit_func_table(out: &mut String, mir: &crate::mir::Mir) {
    let poll_count = mir.functions.iter().filter(|f| f.is_async).count();
    let n = mir.functions.len() + poll_count;
    if n == 0 {
        return;
    }
    let _ = writeln!(out, "(table $__ft {} funcref)", n);
    let mut syms: Vec<String> = mir
        .functions
        .iter()
        .map(|f| format!("${}", func_symbol(f)))
        .collect();
    for f in mir.functions.iter().filter(|f| f.is_async) {
        syms.push(format!("${}", poll_symbol(f)));
    }
    let _ = writeln!(out, "(elem (i32.const 0) {})", syms.join(" "));
    out.push_str("(export \"__indirect_function_table\" (table $__ft))\n");
}

/// Assigns each struct and (discriminated) union a distinct runtime tag, starting at
/// [`STRUCT_TAG_BASE`], in layout-table order (deterministic). The same map drives both the tag
/// stamped at allocation (`New`/`UnionNew`) and the `$object_to_string`/`$print_object` dispatch, so
/// they always agree; the exact numeric value only needs to be self-consistent within a module.
pub(super) fn struct_tags(mir: &crate::mir::Mir) -> HashMap<TypeId, i32> {
    mir.layouts
        .structs
        .keys()
        .chain(mir.layouts.unions.keys())
        .enumerate()
        .map(|(i, ty)| (*ty, STRUCT_TAG_BASE + i as i32))
        .collect()
}

/// The symbol of the dispatch trampoline for method slot `method_slot` of the interface with the
/// stable id `iface_id`. Interface call sites `(call $<sym>)` this trampoline, which performs the
/// tag-indexed itable lookup and forwards through `$__ft`.
pub(super) fn iface_dispatch_symbol(iface_id: usize, method_slot: usize) -> String {
    format!("__iface_dispatch_{}_{}", iface_id, method_slot)
}

/// The emitted linear-memory data + WAT trampolines that implement interface dispatch.
pub(super) struct InterfaceDispatch {
    /// `(data ...)` segments holding the per-interface tag-indexed method tables.
    pub data: String,
    /// The `(func $__iface_dispatch_I_S ...)` trampolines, one per interface method slot.
    pub trampolines: String,
    /// The heap bump-pointer start, past the emitted itable region (8-byte aligned).
    pub heap_start: u32,
}

/// The set of `(iface_id, method_slot)` pairs that some surviving function actually dispatches
/// through. Interface calls appear explicitly as `InterfaceCall` statements/rvalues (sync) or in the
/// preserved HIR body (async), so this is a complete accounting — dispatch trampolines for pairs not
/// listed here are dead and can be skipped.
pub(super) fn used_iface_slots(mir: &crate::mir::Mir) -> std::collections::HashSet<(usize, usize)> {
    use crate::mir::{Rvalue, Statement};
    let mut used = std::collections::HashSet::new();
    for f in &mir.functions {
        for b in &f.blocks {
            for s in &b.stmts {
                match s {
                    Statement::InterfaceCall {
                        iface_id,
                        method_slot,
                        ..
                    } => {
                        used.insert((*iface_id, *method_slot));
                    }
                    Statement::Assign(
                        _,
                        Rvalue::InterfaceCall {
                            iface_id,
                            method_slot,
                            ..
                        },
                    ) => {
                        used.insert((*iface_id, *method_slot));
                    }
                    _ => {}
                }
            }
        }
        if f.is_async {
            if let Some(hir_fn) = &f.hir_fn {
                let mut edges = crate::mir::HirEdges::default();
                crate::mir::hir_body_edges(&hir_fn.body, &mut edges);
                used.extend(edges.iface_calls);
            }
        }
    }
    used
}

/// Builds the interface dispatch machinery: a dense, tag-indexed method table per interface plus a
/// dispatch trampoline per interface method slot.
///
/// Each interface `iid` gets a contiguous `i32` table of `num_tags * method_count` entries laid out
/// in linear memory starting at `itab_base`. Entry `[tag * method_count + slot]` holds the `$__ft`
/// index of the concrete `{Class}_{method}` that the class with runtime `tag` supplies for that
/// interface method slot (0 for tags that do not implement the interface). At a call site the
/// trampoline computes `tag = $object_tag(this)`, loads that entry, and `call_indirect`s it.
pub(super) fn emit_interface_dispatch(
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    itab_base: u32,
    used_slots: &std::collections::HashSet<(usize, usize)>,
) -> InterfaceDispatch {
    let ifaces = &mir.interfaces.interfaces;
    if ifaces.is_empty() {
        return InterfaceDispatch {
            data: String::new(),
            trampolines: String::new(),
            heap_start: itab_base,
        };
    }

    let tags = struct_tags(mir);
    let max_tag = tags.values().copied().max().unwrap_or(STRUCT_TAG_BASE - 1);
    // Dense per-interface tables are sized `num_tags * method_count`; guard the `+1` and the sign so
    // a corrupt (negative/overflowing) tag can't wrap into a huge allocation or truncate the row
    // count. Tags are assigned as small `STRUCT_TAG_BASE + i`, so this only trips on a real bug.
    let num_tags = if max_tag >= 0 {
        (max_tag as usize).saturating_add(1)
    } else {
        0
    };

    // Concrete method symbol -> its `$__ft` slot (the function's position in `mir.functions`).
    let by_symbol: HashMap<&str, usize> = mir
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();

    // One dense [num_tags * method_count] table per interface, filled from each class's impl.
    let mut tables: Vec<Vec<i32>> = ifaces
        .iter()
        .map(|inf| vec![0i32; num_tags * inf.method_count])
        .collect();
    for imp in &mir.interfaces.impls {
        let tag = match tags.get(&imp.class_ty) {
            Some(t) => *t as usize,
            None => continue,
        };
        for (iid, symbols) in &imp.entries {
            let k = ifaces[*iid].method_count;
            for (slot, sym) in symbols.iter().enumerate() {
                if slot >= k {
                    continue;
                }
                // A missing symbol here is an impl entry for a method that whole-module DCE pruned
                // (e.g. a constraint method never dispatched through this interface at runtime). The
                // slot is therefore never reached, so 0 is a harmless filler; keep it lenient rather
                // than trapping on dead-but-listed entries.
                let idx = by_symbol.get(sym.as_str()).copied().unwrap_or(0);
                tables[*iid][tag * k + slot] = idx as i32;
            }
        }
    }

    // Lay the tables out consecutively (4-byte words), recording each interface's base address.
    let mut bases: Vec<u32> = Vec::with_capacity(ifaces.len());
    let mut data = String::new();
    let mut addr = itab_base;
    for table in &tables {
        bases.push(addr);
        if !table.is_empty() {
            let mut bytes = String::new();
            for word in table {
                for b in word.to_le_bytes() {
                    let _ = write!(bytes, "\\{:02x}", b);
                }
            }
            let _ = writeln!(data, "(data (i32.const {}) \"{}\")", addr, bytes);
        }
        addr += (table.len() as u32) * 4;
    }
    let heap_start = (addr.max(itab_base) + 7) & !7;

    // A dispatch trampoline per (interface, method slot): forward the args, look the concrete method
    // up in the interface's tag-indexed table, and `call_indirect` it through `$__ft`.
    let mut trampolines = String::new();
    for (iid, inf) in ifaces.iter().enumerate() {
        let k = inf.method_count;
        for slot in 0..k {
            // Skip trampolines for method slots no surviving call site dispatches through: they are
            // dead code (nothing references the symbol). The itable data above keeps its full layout
            // so runtime indexing of the *used* slots stays correct.
            if !used_slots.contains(&(iid, slot)) {
                continue;
            }
            let (signame, ptys, rty) = match func_sig(interner, inf.sigs[slot]) {
                Some(s) => s,
                None => continue,
            };
            // With the sret ABI the hidden destination pointer is param 0, so the receiver (`this`,
            // used to index the itable) is param 1; otherwise the receiver is param 0.
            let is_sret = func_sig_is_sret(interner, inf.sigs[slot]);
            let recv_idx = if is_sret { 1 } else { 0 };
            let _ = write!(trampolines, "(func ${}", iface_dispatch_symbol(iid, slot));
            for p in &ptys {
                let _ = write!(trampolines, " (param {})", p);
            }
            if let Some(r) = rty {
                let _ = write!(trampolines, " (result {})", r);
            }
            trampolines.push('\n');
            // Push the forwarded call arguments (an sret destination pointer first when present,
            // then the receiver / `this`, then the real args).
            for i in 0..ptys.len() {
                let _ = writeln!(trampolines, "  (local.get {})", i);
            }
            // idx = itable[base + (object_tag(this) * method_count + slot) * 4]
            let _ = writeln!(trampolines, "  (local.get {})", recv_idx);
            let _ = writeln!(trampolines, "  (call $object_tag)");
            let _ = writeln!(trampolines, "  (i32.const {}) (i32.mul)", k);
            let _ = writeln!(trampolines, "  (i32.const {}) (i32.add)", slot);
            let _ = writeln!(trampolines, "  (i32.const 2) (i32.shl)");
            let _ = writeln!(trampolines, "  (i32.const {}) (i32.add)", bases[iid]);
            let _ = writeln!(trampolines, "  (i32.load)");
            let _ = writeln!(trampolines, "  (call_indirect $__ft (type {}))", signame);
            let _ = writeln!(trampolines, ")");
        }
    }

    InterfaceDispatch {
        data,
        trampolines,
        heap_start,
    }
}
