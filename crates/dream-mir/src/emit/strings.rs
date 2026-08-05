use super::*;

/// The fixed runtime strings the object protocol references: the `null`/`<object>` fallbacks plus
/// each struct's default `to_string` pieces (`"Point { "`, `"x: "`, `", y: "`, `" }"`). Interned
/// alongside the program's own literals so `$<Type>_to_string` can reference their data pointers.
pub(super) fn protocol_strings(mir: &crate::Mir) -> Vec<String> {
    let mut v = vec![
        "null".to_string(),
        "<object>".to_string(),
        "[".to_string(),
        "]".to_string(),
        ", ".to_string(),
    ];
    // `length` is the JS array-length key read by the generated `$js_to_array_t*` marshalers.
    v.push("length".to_string());
    for layout in mir.layouts.structs.values() {
        v.push(format!("{} {{ ", layout.name));
        for (i, f) in layout.fields.iter().enumerate() {
            v.push(if i == 0 {
                format!("{}: ", f.name)
            } else {
                format!(", {}: ", f.name)
            });
            // The bare field name is the JS property key used by the struct<->js marshalers.
            v.push(f.name.clone());
        }
        v.push(" }".to_string());
    }
    for layout in mir.layouts.unions.values() {
        for variant in &layout.variants {
            let (prefix, labels, suffix) = union_variant_pieces(variant);
            v.push(prefix);
            v.extend(labels);
            v.push(suffix);
        }
    }
    v
}

/// The `(prefix, field-labels, suffix)` literal pieces of a union variant's `to_string`. Data
/// variants render as `Variant(a: <a>, b: <b>)`; unit variants render as just `Variant`.
pub(super) fn union_variant_pieces(v: &dream_hir::UnionVariant) -> (String, Vec<String>, String) {
    if v.fields.is_empty() {
        return (v.name.clone(), Vec::new(), String::new());
    }
    let prefix = format!("{}(", v.name);
    let labels = v
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            if i == 0 {
                format!("{}: ", f.name)
            } else {
                format!(", {}: ", f.name)
            }
        })
        .collect();
    (prefix, labels, ")".to_string())
}

/// Interns every string constant in the program to a data pointer, in first-appearance order
/// (deterministic). Each string is a heap-object block
/// `[size=0][tag=STRING][ref_count=1][len: i32][utf8]`; the mapped address points at the length
/// word (block start + [`HEAP_HEADER_SIZE`]), with the utf8 bytes at `ptr+4`, so it is a valid
/// runtime string pointer. There is no NUL terminator (the length prefix makes it redundant). Blocks
/// are laid out consecutively, 4-byte aligned.
///
/// When `locate_panics` is true (debug / debug-info builds), every checked site gets a unique
/// file:line panic message. Release builds pass false and intern only the four shared base
/// messages — typically saving many kilobytes of data-section bloat.
pub(super) fn string_table(
    mir: &crate::Mir,
    interner: &TypeInterner,
    locate_panics: bool,
) -> IndexMap<String, u32> {
    let mut found = Vec::new();
    let mut panic_msgs: Vec<String> = Vec::new();
    if locate_panics {
        for f in &mir.functions {
            for b in &f.blocks {
                for s in &b.stmts {
                    strings_in_stmt(s, &mut found);
                }
                strings_in_terminator(&b.terminator, &mut found);
            }
            // An async function's MIR body is a stub: the real body (with the real `SourceLine`
            // markers and checked constructs) is rebuilt from `hir_fn` by the coroutine transform, at
            // emission time (see `emit_async_poll`). Lower it here too — exactly like
            // `debug_map::DebugModule::build` already does for the debug-info source map — so its
            // string literals *and* located panic messages get pre-interned identically to a plain
            // function's, instead of falling back to the (real-line-less) `line == 0` triple.
            if f.is_async {
                if let Some(hir_fn) = &f.hir_fn {
                    let mut edges = crate::HirEdges::default();
                    crate::hir_body_edges(&hir_fn.body, &mut edges);
                    found.extend(edges.strings);
                    let poll_body = crate::lower::lower_async_poll_body(hir_fn, interner);
                    panic_msgs.extend(located_panics_in_function(&poll_body));
                }
            } else {
                panic_msgs.extend(located_panics_in_function(f));
            }
        }
    } else {
        for f in &mir.functions {
            for b in &f.blocks {
                for s in &b.stmts {
                    strings_in_stmt(s, &mut found);
                }
                strings_in_terminator(&b.terminator, &mut found);
            }
            if f.is_async {
                if let Some(hir_fn) = &f.hir_fn {
                    let mut edges = crate::HirEdges::default();
                    crate::hir_body_edges(&hir_fn.body, &mut edges);
                    found.extend(edges.strings);
                }
            }
        }
        panic_msgs.extend(panic_msgs::ALL.iter().map(|s| (*s).to_string()));
    }
    let mut map: IndexMap<String, u32> = IndexMap::new();
    let mut block = STRING_BASE;
    // Seed the constants the `*_to_string`/object-protocol runtime references so they always have
    // stable addresses, regardless of which literals the program itself uses.
    let found = RUNTIME_STR_CONSTS
        .iter()
        .map(|s| s.to_string())
        .chain(panic_msgs)
        .chain(protocol_strings(mir))
        .chain(found);
    for s in found {
        if !map.contains_key(&s) {
            // 12-byte heap header + 4-byte length prefix + utf8 bytes (no NUL terminator).
            let total = HEAP_HEADER_SIZE + 4 + s.len() as u32;
            map.insert(s, block + HEAP_HEADER_SIZE);
            block += (total + 3) & !3;
        }
    }
    map
}

pub(super) fn strings_in_operand(op: &Operand, out: &mut Vec<String>) {
    match op {
        Operand::Const(Const::Str(s)) => out.push(s.clone()),
        Operand::Copy(Place::Index { index, .. }) => strings_in_operand(index, out),
        _ => {}
    }
}

pub(super) fn strings_in_rvalue(rv: &Rvalue, out: &mut Vec<String>) {
    match rv {
        Rvalue::Select {
            cond,
            then_val,
            else_val,
        } => {
            strings_in_operand(cond, out);
            strings_in_operand(then_val, out);
            strings_in_operand(else_val, out);
        }
        Rvalue::Use(o)
        | Rvalue::Unary(_, o)
        | Rvalue::ArrayLen(o)
        | Rvalue::StrLen(o)
        | Rvalue::Cast(o, _, _)
        | Rvalue::IsType(o, _)
        | Rvalue::Discriminant(o)
        | Rvalue::UnionField { base: o, .. } => strings_in_operand(o, out),
        Rvalue::Binary(_, a, b) | Rvalue::CharAt(a, b) | Rvalue::Concat(a, b) => {
            strings_in_operand(a, out);
            strings_in_operand(b, out);
        }
        Rvalue::ArrayNew { len, .. } => strings_in_operand(len, out),
        Rvalue::ToBytes { value: o, .. } | Rvalue::FromBytes { bytes: o, .. } => {
            strings_in_operand(o, out)
        }
        Rvalue::ArrayRealloc { array, new_len, .. } => {
            strings_in_operand(array, out);
            strings_in_operand(new_len, out);
        }
        Rvalue::HashCode(o) | Rvalue::ToString(o) => strings_in_operand(o, out),
        Rvalue::EnumName { value, arms } => {
            strings_in_operand(value, out);
            out.push(String::new());
            arms.iter().for_each(|(_, name)| out.push(name.clone()));
        }
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::UnionNew { args, .. }
        | Rvalue::ArrayLit { elems: args, .. } => {
            args.iter().for_each(|a| strings_in_operand(a, out))
        }
        Rvalue::IndirectCall { target, args } => {
            strings_in_operand(target, out);
            args.iter().for_each(|a| strings_in_operand(a, out));
        }
        Rvalue::InterfaceCall { receiver, args, .. } => {
            strings_in_operand(receiver, out);
            args.iter().for_each(|a| strings_in_operand(a, out));
        }
        Rvalue::JsCall {
            target,
            method,
            args,
            ..
        } => {
            strings_in_operand(target, out);
            if let Some(m) = method {
                strings_in_operand(m, out);
            }
            args.iter().for_each(|(a, _)| strings_in_operand(a, out));
        }
        Rvalue::FuncRef(_) => {}
    }
}

pub(super) fn strings_in_stmt(s: &Statement, out: &mut Vec<String>) {
    match s {
        Statement::Assign(place, rv) => {
            if let Place::Index { index, .. } = place {
                strings_in_operand(index, out);
            }
            strings_in_rvalue(rv, out);
        }
        Statement::Retain(o) | Statement::Release(o) | Statement::Panic(o) => {
            strings_in_operand(o, out)
        }
        Statement::Call { args, .. } => args.iter().for_each(|a| strings_in_operand(a, out)),
        Statement::InterfaceCall { receiver, args, .. } => {
            strings_in_operand(receiver, out);
            args.iter().for_each(|a| strings_in_operand(a, out));
        }
        Statement::IndirectCall { target, args } => {
            strings_in_operand(target, out);
            args.iter().for_each(|a| strings_in_operand(a, out));
        }
        Statement::Print { arg, .. } => strings_in_operand(arg, out),
        Statement::ForceFree(o) => strings_in_operand(o, out),
        Statement::LockAcquire(o) | Statement::LockRelease(o) => strings_in_operand(o, out),
        Statement::Nop | Statement::DebugLine(_) | Statement::SourceLine(_) => {}
    }
}

/// Which located panic message bases (if any) `s` will look up in [`Emitter::emit_panic`] once
/// emitted, at the current source line. Mirrors, as a pure prediction, exactly which MIR shapes the
/// backend (`Emitter::emit_bounds_check`/`emit_char_at`, the `Div`/`Rem` check in `emit_rvalue`, the
/// unbox-cast check in `emit_cast`) actually turns into an `emit_panic` call — so
/// [`located_panics_in_function`] can pre-intern precisely the strings emission will ask for.
/// Deliberately over-approximates rather than under-approximates where the exact backend condition
/// depends on more than the MIR shape alone (e.g. `emit_cast` only panics for some `(from, to)`
/// pairs, not literally every `Rvalue::Cast`): an unused interned string is harmless, a missing one
/// is a hard `internal_error!` crash in [`Emitter::string_addr`].
fn checked_bases_in_stmt(s: &Statement, out: &mut Vec<&'static str>) {
    fn in_place(p: &Place, out: &mut Vec<&'static str>) {
        match p {
            Place::Index { index, .. } => {
                out.push(panic_msgs::INDEX_OUT_OF_BOUNDS);
                in_operand(index, out);
            }
            // A field *read* may be an `unowned` field, in which case
            // `Emitter::emit_unowned_read_check` emits a located `UNOWNED_NULL_DEREF` panic; whether
            // the field is actually `unowned` depends on layout info not available here, so this
            // over-approximates (pre-interns the message for every field read).
            Place::Field { .. } => out.push(panic_msgs::UNOWNED_NULL_DEREF),
            Place::Local(_) | Place::Global(_) => {}
        }
    }
    fn in_operand(o: &Operand, out: &mut Vec<&'static str>) {
        if let Operand::Copy(p) = o {
            in_place(p, out);
        }
    }
    fn in_rvalue(rv: &Rvalue, out: &mut Vec<&'static str>) {
        match rv {
            Rvalue::Binary(op, a, b) => {
                if matches!(op, BinOp::Div | BinOp::Rem) {
                    out.push(panic_msgs::DIVIDE_BY_ZERO);
                }
                in_operand(a, out);
                in_operand(b, out);
            }
            Rvalue::CharAt(a, b) => {
                out.push(panic_msgs::INDEX_OUT_OF_BOUNDS);
                in_operand(a, out);
                in_operand(b, out);
            }
            Rvalue::Cast(o, _, _) => {
                out.push(panic_msgs::INVALID_CAST);
                in_operand(o, out);
            }
            Rvalue::Select {
                cond,
                then_val,
                else_val,
            } => {
                in_operand(cond, out);
                in_operand(then_val, out);
                in_operand(else_val, out);
            }
            Rvalue::Use(o)
            | Rvalue::Unary(_, o)
            | Rvalue::ArrayLen(o)
            | Rvalue::StrLen(o)
            | Rvalue::IsType(o, _)
            | Rvalue::Discriminant(o)
            | Rvalue::UnionField { base: o, .. } => in_operand(o, out),
            Rvalue::Concat(a, b) => {
                in_operand(a, out);
                in_operand(b, out);
            }
            Rvalue::ArrayNew { len, .. } => in_operand(len, out),
            Rvalue::ToBytes { value: o, .. } | Rvalue::FromBytes { bytes: o, .. } => {
                in_operand(o, out)
            }
            Rvalue::ArrayRealloc { array, new_len, .. } => {
                in_operand(array, out);
                in_operand(new_len, out);
            }
            Rvalue::HashCode(o) | Rvalue::ToString(o) => in_operand(o, out),
            Rvalue::EnumName { value, .. } => in_operand(value, out),
            Rvalue::Call { args, .. }
            | Rvalue::New { args, .. }
            | Rvalue::UnionNew { args, .. }
            | Rvalue::ArrayLit { elems: args, .. } => args.iter().for_each(|a| in_operand(a, out)),
            Rvalue::IndirectCall { target, args } => {
                in_operand(target, out);
                args.iter().for_each(|a| in_operand(a, out));
            }
            Rvalue::InterfaceCall { receiver, args, .. } => {
                in_operand(receiver, out);
                args.iter().for_each(|a| in_operand(a, out));
            }
            Rvalue::JsCall { target, args, .. } => {
                in_operand(target, out);
                args.iter().for_each(|(a, _)| in_operand(a, out));
            }
            Rvalue::FuncRef(_) => {}
        }
    }
    match s {
        Statement::Assign(place, rv) => {
            in_place(place, out);
            in_rvalue(rv, out);
        }
        Statement::Retain(_)
        | Statement::Release(_)
        | Statement::Panic(_)
        | Statement::Call { .. }
        | Statement::InterfaceCall { .. }
        | Statement::IndirectCall { .. }
        | Statement::Print { .. }
        | Statement::ForceFree(_)
        | Statement::LockAcquire(_)
        | Statement::LockRelease(_)
        | Statement::Nop
        | Statement::DebugLine(_)
        | Statement::SourceLine(_) => {}
    }
}

/// Every located panic message [`located_panics_in_function`] needs pre-interned for `func`.
/// Collects every `SourceLine` value and every checked-construct base in the function, then
/// interns the Cartesian product (plus the `line == 0` safety net). Visit order must not matter:
/// sync shape emit walks blocks in relooper-shape order, which differs from raw block-index order,
/// so a running `current_line` scan would disagree with emission.
fn located_panics_in_function(f: &MirFunction) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut out: Vec<String> = panic_msgs::located_all(f.file.as_deref(), &f.name, 0).to_vec();
    let mut lines: BTreeSet<u32> = BTreeSet::new();
    lines.insert(0);
    let mut bases: BTreeSet<String> = BTreeSet::new();
    let mut tmp = Vec::new();
    for b in &f.blocks {
        for s in &b.stmts {
            if let Statement::SourceLine(l) = s {
                lines.insert(*l);
            }
            tmp.clear();
            checked_bases_in_stmt(s, &mut tmp);
            for base in &tmp {
                bases.insert((*base).to_string());
            }
        }
    }
    for line in lines {
        for base in &bases {
            out.push(panic_msgs::located(
                base,
                f.file.as_deref(),
                &f.name,
                line,
            ));
        }
    }
    out
}

pub(super) fn strings_in_terminator(t: &Terminator, out: &mut Vec<String>) {
    match t {
        Terminator::If { cond, .. } => strings_in_operand(cond, out),
        Terminator::Switch { value, .. } => strings_in_operand(value, out),
        Terminator::Return(Some(o)) => strings_in_operand(o, out),
        Terminator::AsyncComplete(Some(o)) => strings_in_operand(o, out),
        _ => {}
    }
}

/// Escapes an interned string's full heap-block bytes as `\HH` pairs: the 12-byte header
/// (`size=0`, `tag=STRING`, `ref_count=1`, little-endian i32s), the length prefix (`len` as a
/// little-endian i32), then the utf8 bytes. No NUL terminator (the length prefix makes it redundant).
/// Written at the block start (the mapped address minus [`HEAP_HEADER_SIZE`]); the mapped address
/// itself points at the length word.
pub(super) fn escape_data(s: &str) -> String {
    let mut out = String::new();
    for word in [0_i32, STRING_TAG, 1, s.len() as i32] {
        for b in word.to_le_bytes() {
            let _ = write!(out, "\\{:02x}", b);
        }
    }
    for b in s.bytes() {
        let _ = write!(out, "\\{:02x}", b);
    }
    out
}
