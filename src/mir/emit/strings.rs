use super::*;

/// The fixed runtime strings the object protocol references: the `null`/`<object>` fallbacks plus
/// each struct's default `to_string` pieces (`"Point { "`, `"x: "`, `", y: "`, `" }"`). Interned
/// alongside the program's own literals so `$<Type>_to_string` can reference their data pointers.
pub(super) fn protocol_strings(mir: &crate::mir::Mir) -> Vec<String> {
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
pub(super) fn union_variant_pieces(v: &crate::hir::UnionVariant) -> (String, Vec<String>, String) {
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
pub(super) fn string_table(mir: &crate::mir::Mir) -> IndexMap<String, u32> {
    let mut found = Vec::new();
    for f in &mir.functions {
        for b in &f.blocks {
            for s in &b.stmts {
                strings_in_stmt(s, &mut found);
            }
            strings_in_terminator(&b.terminator, &mut found);
        }
        // An async function's MIR body is a stub; its literals live in the preserved HIR snapshot
        // (emitted later by the coroutine transform), so harvest them here too — otherwise those
        // string constants get no data segment and lower to a null pointer.
        if f.is_async {
            if let Some(hir_fn) = &f.hir_fn {
                let mut edges = crate::mir::HirEdges::default();
                crate::mir::hir_body_edges(&hir_fn.body, &mut edges);
                found.extend(edges.strings);
            }
        }
    }
    let mut map: IndexMap<String, u32> = IndexMap::new();
    let mut block = STRING_BASE;
    // Seed the constants the `*_to_string`/object-protocol runtime references so they always have
    // stable addresses, regardless of which literals the program itself uses.
    let found = RUNTIME_STR_CONSTS
        .iter()
        .map(|s| s.to_string())
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
        Statement::Retain(o) | Statement::Release(o) => strings_in_operand(o, out),
        Statement::Call { args, .. } => args.iter().for_each(|a| strings_in_operand(a, out)),
        Statement::InterfaceCall { receiver, args, .. } => {
            strings_in_operand(receiver, out);
            args.iter().for_each(|a| strings_in_operand(a, out));
        }
        Statement::Print { arg, .. } => strings_in_operand(arg, out),
        Statement::Nop | Statement::DebugLine(_) => {}
    }
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
