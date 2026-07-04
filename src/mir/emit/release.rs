use super::*;

/// The `$release_*` symbol that deep-releases a reference value of `ty` (chosen *statically* from the
/// declared type): structs/unions call their generated per-type release, reference-element arrays
/// their element-typed array release, and everything else (strings, scalar arrays, boxed primitives)
/// drops one reference via the generic runtime. `object`-typed values route through the tag-dispatched
/// `$release_object` since their concrete type is unknown until runtime. Callers guard on
/// [`TypeInterner::is_reference`] first, so non-reference types never reach here.
pub(super) fn release_call(interner: &TypeInterner, layouts: &LayoutTable, ty: TypeId) -> String {
    let ty = interner.strip_nullable(ty);
    match interner.kind(ty) {
        TyKind::Struct(..) | TyKind::Union(..) => {
            if let Some(l) = layouts.structs.get(&ty) {
                format!("$release_{}", l.name)
            } else if let Some(l) = layouts.unions.get(&ty) {
                format!("$release_{}", l.name)
            } else {
                "$release_object".to_string()
            }
        }
        TyKind::Array(e) if interner.is_reference(*e) || interner.is_value_type(*e) => {
            format!("$release_array_t{}", e.0)
        }
        // An interface-typed value is a concrete tagged object; release it through the
        // tag-dispatching `$release_object` so the concrete type's deep release runs.
        TyKind::Object | TyKind::Interface(..) => "$release_object".to_string(),
        _ => "$release_generic".to_string(),
    }
}

/// Emits the null check + refcount decrement shared by every per-type release, opening the
/// `if (new_count == 0) (then` block that the caller fills with the deep-release + `$free`. Uses only
/// the `$rc`/`$nc` locals, which every release function declares. Matches `$release_generic`'s ABI
/// (refcount word at `ptr - 4`).
pub(super) fn emit_release_prologue(out: &mut String) {
    out.push_str("  (local.get $ptr) (i32.eqz) (if (then (return)))\n");
    out.push_str("  (local.get $ptr) (i32.const 4) (i32.sub) (local.set $rc)\n");
    out.push_str("  (local.get $rc) (i32.load) (i32.const 1) (i32.sub) (local.set $nc)\n");
    out.push_str("  (local.get $rc) (local.get $nc) (i32.store)\n");
    out.push_str("  (local.get $nc) (i32.eqz) (if (then\n");
}

/// Emits the `del()` destructor invocation (when the type declares one): the refcount is first pinned
/// to 1 so the destructor body's own `this` retain/release cannot re-enter this release at zero, then
/// `$<Type>_del(ptr)` runs while the fields are still live. `del` is the destructor's function symbol
/// or `None`.
pub(super) fn emit_del_call(out: &mut String, del: Option<&str>) {
    if let Some(d) = del {
        out.push_str("    (local.get $rc) (i32.const 1) (i32.store)\n");
        let _ = writeln!(out, "    (local.get $ptr) (call ${})", d);
    }
}

/// Emits an in-place drop of an inline value(`struct`) field `f` (at `ptr + offset`) via its
/// `$__vs_drop_<T>` glue, if the field is a value struct that requires glue. A no-op otherwise. The
/// inline storage is not freed (it belongs to the enclosing object's block).
fn emit_embedded_value_drop(
    out: &mut String,
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    value_glue: &std::collections::HashSet<TypeId>,
    f: &crate::hir::FieldLayout,
    indent: &str,
) {
    if !interner.is_value_type(f.ty) {
        return;
    }
    let stripped = interner.strip_nullable(f.ty);
    if !value_glue.contains(&stripped) {
        return;
    }
    let Some(name) = mir.layouts.structs.get(&stripped).map(|l| l.name.clone()) else {
        return;
    };
    out.push_str(indent);
    out.push_str("(local.get $ptr)");
    if f.offset > 0 {
        let _ = write!(out, " (i32.const {}) (i32.add)", f.offset);
    }
    let _ = writeln!(out, " (call {})", vs_drop_sym(&name));
}

/// Emits the deep-release runtime: a per-struct/union `$release_<Type>` (run `del()` if present,
/// release reference fields, then `$free`), a `$release_array_t<E>` for each reference-element array
/// type, and the tag-dispatching `$release_object`. Non-reference fields and scalar arrays never need
/// releasing; strings/boxed primitives fall through to `$release_generic`.
pub(super) fn emit_release_funcs(
    out: &mut String,
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    tags: &HashMap<TypeId, i32>,
    value_glue: &std::collections::HashSet<TypeId>,
) {
    let fn_names: std::collections::HashSet<&str> =
        mir.functions.iter().map(|f| f.name.as_str()).collect();
    let del_of = |name: &str| -> Option<String> {
        let sym = format!("{}_del", name);
        fn_names.contains(sym.as_str()).then_some(sym)
    };

    for layout in mir.layouts.structs.values() {
        let del = del_of(&layout.name);
        let _ = writeln!(out, "(func $release_{} (param $ptr i32)", layout.name);
        out.push_str("  (local $rc i32) (local $nc i32)\n");
        emit_release_prologue(out);
        emit_del_call(out, del.as_deref());
        for f in layout.fields.iter().filter(|f| interner.is_reference(f.ty)) {
            out.push_str("    (local.get $ptr)\n");
            if f.offset > 0 {
                let _ = writeln!(out, "    (i32.const {}) (i32.add)", f.offset);
            }
            let _ = writeln!(
                out,
                "    (i32.load) (call {})",
                release_call(interner, &mir.layouts, f.ty)
            );
        }
        // Inline value(`struct`) fields are dropped in place (their reference fields released, `del`
        // run) via the field type's drop-glue at `ptr + offset` — never freed, since they share the
        // enclosing object's block.
        for f in &layout.fields {
            emit_embedded_value_drop(out, mir, interner, value_glue, f, "    ");
        }
        out.push_str("    (local.get $ptr) (call $free)\n  ))\n)\n");
    }

    for layout in mir.layouts.unions.values() {
        let del = del_of(&layout.name);
        let _ = writeln!(out, "(func $release_{} (param $ptr i32)", layout.name);
        out.push_str("  (local $rc i32) (local $nc i32) (local $d i32)\n");
        emit_release_prologue(out);
        emit_del_call(out, del.as_deref());
        // Only the active variant's payload is valid, so switch on the discriminant (offset 0).
        out.push_str("    (local.get $ptr) (i32.load) (local.set $d)\n");
        for v in &layout.variants {
            let ref_fields: Vec<&crate::hir::FieldLayout> = v
                .fields
                .iter()
                .filter(|f| interner.is_reference(f.ty))
                .collect();
            if ref_fields.is_empty() {
                continue;
            }
            let _ = writeln!(
                out,
                "    (local.get $d) (i32.const {}) (i32.eq) (if (then",
                v.discriminant
            );
            for f in ref_fields {
                out.push_str("      (local.get $ptr)\n");
                if f.offset > 0 {
                    let _ = writeln!(out, "      (i32.const {}) (i32.add)", f.offset);
                }
                let _ = writeln!(
                    out,
                    "      (i32.load) (call {})",
                    release_call(interner, &mir.layouts, f.ty)
                );
            }
            out.push_str("    ))\n");
        }
        // Inline value-struct payloads (present in at most one active variant) are dropped in place.
        for v in &layout.variants {
            let value_fields: Vec<&crate::hir::FieldLayout> = v
                .fields
                .iter()
                .filter(|f| {
                    interner.is_value_type(f.ty)
                        && value_glue.contains(&interner.strip_nullable(f.ty))
                })
                .collect();
            if value_fields.is_empty() {
                continue;
            }
            let _ = writeln!(
                out,
                "    (local.get $d) (i32.const {}) (i32.eq) (if (then",
                v.discriminant
            );
            for f in value_fields {
                emit_embedded_value_drop(out, mir, interner, value_glue, f, "      ");
            }
            out.push_str("    ))\n");
        }
        out.push_str("    (local.get $ptr) (call $free)\n  ))\n)\n");
    }

    // One array release per reference- or value-element array type; the element type is known
    // statically at the call site, so array releases (unlike `$release_object`) can recurse into
    // their elements. Reference elements are released by loaded pointer; inline value-struct elements
    // are dropped in place via their drop-glue at the element address.
    for elem in array_elem_types(mir, interner) {
        let is_ref = interner.is_reference(elem);
        let is_value = interner.is_value_type(elem);
        if !is_ref && !is_value {
            continue;
        }
        let _ = writeln!(out, "(func $release_array_t{} (param $ptr i32)", elem.0);
        out.push_str(
            "  (local $rc i32) (local $nc i32) (local $len i32) (local $i i32) (local $elem i32)\n",
        );
        emit_release_prologue(out);
        if is_ref {
            out.push_str("    (local.get $ptr) (i32.load) (local.set $len)\n");
            out.push_str("    (i32.const 0) (local.set $i)\n");
            out.push_str("    (block $done (loop $scan\n");
            out.push_str("      (local.get $i) (local.get $len) (i32.ge_s) (br_if $done)\n");
            out.push_str("      (local.get $ptr) (i32.const 4) (i32.add) (local.get $i) (i32.const 4) (i32.mul) (i32.add) (i32.load) (local.set $elem)\n");
            let _ = writeln!(
                out,
                "      (local.get $elem) (if (then (local.get $elem) (call {})))",
                release_call(interner, &mir.layouts, elem)
            );
            out.push_str(
                "      (local.get $i) (i32.const 1) (i32.add) (local.set $i) (br $scan)))\n",
            );
        } else if value_glue.contains(&interner.strip_nullable(elem)) {
            // Drop each inline element (stride = its inline size) via `$__vs_drop_<T>` at its address.
            let name = mir.layouts.structs[&interner.strip_nullable(elem)]
                .name
                .clone();
            let (stride, _) = crate::hir::scalar_size(interner, elem);
            out.push_str("    (local.get $ptr) (i32.load) (local.set $len)\n");
            out.push_str("    (i32.const 0) (local.set $i)\n");
            out.push_str("    (block $done (loop $scan\n");
            out.push_str("      (local.get $i) (local.get $len) (i32.ge_s) (br_if $done)\n");
            let _ = writeln!(
                out,
                "      (local.get $ptr) (i32.const 4) (i32.add) (local.get $i) (i32.const {}) (i32.mul) (i32.add) (call {})",
                stride,
                vs_drop_sym(&name)
            );
            out.push_str(
                "      (local.get $i) (i32.const 1) (i32.add) (local.set $i) (br $scan)))\n",
            );
        }
        out.push_str("    (local.get $ptr) (call $free)\n  ))\n)\n");
    }

    // `$release_object`: tag dispatch for reference values whose static type is `object`. Strings,
    // boxed primitives, and arrays (not self-describing about their element type) fall through to the
    // shallow generic release.
    out.push_str("(func $release_object (param $ptr i32)\n  (local $tag i32)\n");
    out.push_str("  (local.get $ptr) (i32.eqz) (if (then (return)))\n");
    out.push_str("  (local.get $ptr) (call $object_tag) (local.set $tag)\n");
    for (ty, layout) in &mir.layouts.structs {
        if let Some(&tag) = tags.get(ty) {
            let _ = writeln!(
                out,
                "  (local.get $tag) (i32.const {}) (i32.eq) (if (then (local.get $ptr) (call $release_{}) (return)))",
                tag, layout.name
            );
        }
    }
    for (ty, layout) in &mir.layouts.unions {
        if let Some(&tag) = tags.get(ty) {
            let _ = writeln!(
                out,
                "  (local.get $tag) (i32.const {}) (i32.eq) (if (then (local.get $ptr) (call $release_{}) (return)))",
                tag, layout.name
            );
        }
    }
    out.push_str("  (local.get $ptr) (call $release_generic)\n)\n");
}
