use super::*;

/// Emits the object-protocol runtime that depends on the user's types: one default `$<Type>_to_string`
/// per struct, plus the tag-dispatching `$object_to_string` and `$print_object` routers. Struct
/// `to_string` renders as `Type { field: value, ... }`, recursing into reference fields via
/// `$object_to_string`.
pub(super) fn emit_object_protocol(
    out: &mut String,
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
) {
    // A user `@override to_string`/`hash_code` is emitted as `$<Type>_{method}`; skip the generated
    // default for those so the symbols do not collide.
    let user_syms: std::collections::HashSet<String> =
        mir.functions.iter().map(func_symbol).collect();
    let has_override =
        |name: &str, method: &str| user_syms.contains(&format!("{}_{}", name, method));
    for layout in mir.layouts.structs.values() {
        if !has_override(&layout.name, "to_string") {
            emit_struct_to_string(out, layout, interner, strings);
        }
    }
    for layout in mir.layouts.unions.values() {
        if !has_override(&layout.name, "to_string") {
            emit_union_to_string(out, layout, interner, strings);
        }
    }
    for elem in array_elem_types(mir, interner) {
        emit_array_to_string(out, elem, interner, strings);
    }
    emit_object_to_string(out, mir, strings, tags);
    // `$print_object`: render via the tag dispatcher, then print the resulting string.
    out.push_str(
        "(func $print_object (param $ptr i32)\n  (local.get $ptr) (call $object_to_string) (call $print_string))\n",
    );
    for layout in mir.layouts.structs.values() {
        if !has_override(&layout.name, "hash_code") {
            emit_struct_hash_code(out, layout, interner);
        }
    }
    for layout in mir.layouts.unions.values() {
        if !has_override(&layout.name, "hash_code") {
            emit_union_hash_code(out, layout, interner);
        }
    }
    emit_object_hash_code(out, mir, tags);
}

/// The instructions that turn a loaded value of `ty` (already on the stack) into its `i32` hash.
/// Integer-family values (and enums) are their own hash; wider/reference types route through a
/// helper or the tag-dispatching `$object_hash_code`. Mirrors [`value_to_string_call`].
pub(super) fn value_hash_code_instrs(interner: &TypeInterner, ty: TypeId) -> &'static str {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(p) => prim_info(*p).hash,
        TyKind::Enum(_) => "",
        _ => "(call $object_hash_code)",
    }
}

/// Folds one loaded field/element value into the running hash accumulator `$h`
/// (`h = h * 31 + hash(value)`): the value's load + hash instructions are supplied by the caller.
pub(super) fn fold_hash_field(out: &mut String, indent: &str, load: &str, hash: &str) {
    let _ = writeln!(out, "{indent}(local.get $h) (i32.const 31) (i32.mul)");
    let _ = writeln!(out, "{indent}{load} {hash}");
    let _ = writeln!(out, "{indent}(i32.add) (local.set $h)");
}

/// Folds a run of fields (in offset order) into `$h` at `indent`. Shared by struct and union
/// `hash_code`; the union calls it once per variant's fields inside the discriminant guard.
fn emit_hash_fields(
    out: &mut String,
    indent: &str,
    fields: &[crate::hir::FieldLayout],
    interner: &TypeInterner,
) {
    for f in fields {
        let load = field_load_expr(interner, f.offset, f.ty);
        fold_hash_field(out, indent, &load, value_hash_code_instrs(interner, f.ty));
    }
}

/// Appends one field/variant-field to the running `$res` string: `res = res + label + to_string(this
/// [+offset] load)`, all at `indent`. `label` is the interned data address of the label piece (e.g.
/// `"x: "` or `", x: "`). Shared by struct and union `to_string`, which differ only in the label
/// pieces and indentation.
fn emit_to_string_field(
    out: &mut String,
    indent: &str,
    label: u32,
    f: &crate::hir::FieldLayout,
    interner: &TypeInterner,
) {
    let _ = writeln!(
        out,
        "{indent}(local.get $res) (i32.const {label}) (call $concat_strings) (local.set $res)"
    );
    let _ = write!(out, "{indent}(local.get $res)\n{indent}(local.get $this)\n");
    if f.offset > 0 {
        let _ = writeln!(out, "{indent}(i32.const {}) (i32.add)", f.offset);
    }
    let _ = writeln!(out, "{indent}({})", load_instr_for(interner, f.ty));
    if let Some(call) = value_to_string_call(interner, f.ty) {
        let _ = writeln!(out, "{indent}(call {})", call);
    }
    let _ = writeln!(out, "{indent}(call $concat_strings) (local.set $res)");
}

/// Emits one struct's default `$<Type>_hash_code`: `h = 17`, folding each field in offset order.
pub(super) fn emit_struct_hash_code(
    out: &mut String,
    layout: &crate::hir::TypeLayout,
    interner: &TypeInterner,
) {
    let _ = writeln!(
        out,
        "(func ${}_hash_code (param $this i32) (result i32)",
        layout.name
    );
    out.push_str("  (local $h i32)\n  (i32.const 17) (local.set $h)\n");
    emit_hash_fields(out, "  ", &layout.fields, interner);
    out.push_str("  (local.get $h)\n)\n");
}

/// Emits one union's default `$<Union>_hash_code`: seeds the accumulator from the discriminant word
/// (offset 0) and folds the matching variant's fields, so equal values hash equally and different
/// variants/payloads (including field order) diverge.
pub(super) fn emit_union_hash_code(
    out: &mut String,
    layout: &crate::hir::UnionLayout,
    interner: &TypeInterner,
) {
    let _ = writeln!(
        out,
        "(func ${}_hash_code (param $this i32) (result i32)",
        layout.name
    );
    out.push_str("  (local $h i32)\n  (local $d i32)\n");
    out.push_str("  (local.get $this) (i32.load) (local.set $d)\n");
    // h = 17 * 31 + discriminant
    out.push_str(
        "  (i32.const 17) (i32.const 31) (i32.mul) (local.get $d) (i32.add) (local.set $h)\n",
    );
    for variant in &layout.variants {
        let _ = writeln!(
            out,
            "  (local.get $d) (i32.const {}) (i32.eq) (if (then",
            variant.discriminant
        );
        emit_hash_fields(out, "    ", &variant.fields, interner);
        out.push_str("  ))\n");
    }
    out.push_str("  (local.get $h)\n)\n");
}

/// The `(local.get $this) [+offset] (load)` expression that reads a field/variant slot of type `ty`.
pub(super) fn field_load_expr(interner: &TypeInterner, offset: u32, ty: TypeId) -> String {
    let add = if offset > 0 {
        format!(" (i32.const {}) (i32.add)", offset)
    } else {
        String::new()
    };
    format!(
        "(local.get $this){} ({})",
        add,
        load_instr_for(interner, ty)
    )
}

/// Emits the tag-dispatching `$object_hash_code`: unbox+hash for boxed primitives, `$hash_string`
/// for strings, and each struct/union's `$<Type>_hash_code` by type tag. Mirrors
/// [`emit_object_to_string`]. A null pointer hashes to 0.
pub(super) fn emit_object_hash_code(
    out: &mut String,
    mir: &crate::mir::Mir,
    tags: &HashMap<TypeId, i32>,
) {
    out.push_str("(func $object_hash_code (param $ptr i32) (result i32)\n  (local $tag i32)\n");
    out.push_str("  (local.get $ptr) (i32.eqz) (if (then (i32.const 0) (return)))\n");
    out.push_str("  (local.get $ptr) (call $object_tag) (local.set $tag)\n");
    // Boxed primitives: unbox then hash; `string` is its own pointer and hashes directly.
    for e in PRIM_TABLE {
        let body = match e.unbox_fn {
            Some(unbox) => format!("(local.get $ptr) (call {}) {}", unbox, e.hash),
            None => format!("(local.get $ptr) {}", e.hash),
        };
        write_tag_arm(out, e.tag, &body);
    }
    write_struct_union_tag_arms(out, mir, tags, |name| {
        format!("(local.get $ptr) (call ${}_hash_code)", name)
    });
    // Unknown/opaque reference: hash by identity (the pointer itself).
    out.push_str("  (local.get $ptr)\n)\n");
}

/// Emits one struct's default `$<Type>_to_string`, concatenating the interned label pieces with each
/// field's rendered value (in offset order).
pub(super) fn emit_struct_to_string(
    out: &mut String,
    layout: &crate::hir::TypeLayout,
    interner: &TypeInterner,
    strings: &IndexMap<String, u32>,
) {
    let prefix = format!("{} {{ ", layout.name);
    let _ = writeln!(
        out,
        "(func ${}_to_string (param $this i32) (result i32)",
        layout.name
    );
    out.push_str("  (local $res i32)\n");
    let _ = writeln!(out, "  (i32.const {}) (local.set $res)", strings[&prefix]);
    for (i, f) in layout.fields.iter().enumerate() {
        let label = if i == 0 {
            format!("{}: ", f.name)
        } else {
            format!(", {}: ", f.name)
        };
        emit_to_string_field(out, "  ", strings[&label], f, interner);
    }
    let _ = writeln!(
        out,
        "  (local.get $res) (i32.const {}) (call $concat_strings)",
        strings[" }"]
    );
    out.push_str(")\n");
}

/// Emits one union's default `$<Union>_to_string`: reads the discriminant word (offset 0) and, for
/// the matching variant, renders `Variant(field: value, ...)` (unit variants render as just the
/// variant name). An unrecognized discriminant falls back to `"<object>"`.
pub(super) fn emit_union_to_string(
    out: &mut String,
    layout: &crate::hir::UnionLayout,
    interner: &TypeInterner,
    strings: &IndexMap<String, u32>,
) {
    let _ = writeln!(
        out,
        "(func ${}_to_string (param $this i32) (result i32)",
        layout.name
    );
    out.push_str("  (local $res i32)\n  (local $d i32)\n");
    let _ = writeln!(
        out,
        "  (i32.const {}) (local.set $res)",
        strings["<object>"]
    );
    out.push_str("  (local.get $this) (i32.load) (local.set $d)\n");
    for variant in &layout.variants {
        let (prefix, labels, suffix) = union_variant_pieces(variant);
        let _ = writeln!(
            out,
            "  (local.get $d) (i32.const {}) (i32.eq) (if (then",
            variant.discriminant
        );
        let _ = writeln!(out, "    (i32.const {}) (local.set $res)", strings[&prefix]);
        for (idx, f) in variant.fields.iter().enumerate() {
            emit_to_string_field(out, "    ", strings[&labels[idx]], f, interner);
        }
        let _ = writeln!(
            out,
            "    (local.get $res) (i32.const {}) (call $concat_strings) (local.set $res)",
            strings[&suffix]
        );
        out.push_str("  ))\n");
    }
    out.push_str("  (local.get $res)\n)\n");
}

/// The distinct array **element** types that need a generated `$array_to_string_t<id>`: those
/// reachable as an array-typed struct/union field, local, global, or a direct `print` of an array.
/// Element types that are themselves arrays are added transitively (fixpoint), so nested arrays render
/// (and deep-release) their contents.
pub(super) fn array_elem_types(mir: &crate::mir::Mir, interner: &TypeInterner) -> Vec<TypeId> {
    let mut order: Vec<TypeId> = Vec::new();
    for layout in mir.layouts.structs.values() {
        for f in &layout.fields {
            push_array_elem(&mut order, interner, f.ty);
        }
    }
    for layout in mir.layouts.unions.values() {
        for v in &layout.variants {
            for f in &v.fields {
                push_array_elem(&mut order, interner, f.ty);
            }
        }
    }
    for f in &mir.functions {
        // Any array-typed local can be printed *or* deep-released, both of which need its element
        // helper; covering all locals keeps `$release_array_t<E>`/`$array_to_string_t<E>` references
        // resolvable even for arrays that are only released (never printed).
        for l in &f.locals {
            push_array_elem(&mut order, interner, l.ty);
        }
        for b in &f.blocks {
            for s in &b.stmts {
                if let Statement::Print { ty, .. } = s {
                    push_array_elem(&mut order, interner, *ty);
                }
            }
        }
    }
    for g in &mir.globals {
        push_array_elem(&mut order, interner, g.ty);
    }
    // Fixpoint: an element type that is *itself* an array (`int[][]` → element `int[]`) needs its own
    // inner-element helper; `push_array_elem` unwraps one array level, so re-pushing each element adds it.
    let mut i = 0;
    while i < order.len() {
        let cur = order[i];
        push_array_elem(&mut order, interner, cur);
        i += 1;
    }
    order
}

/// If `ty` (after nullable stripping) is an array, records its element type in `order` (dedup,
/// first-seen order).
pub(super) fn push_array_elem(order: &mut Vec<TypeId>, interner: &TypeInterner, ty: TypeId) {
    if let Some(e) = interner.unwrap_array(interner.strip_nullable(ty)) {
        if !order.contains(&e) {
            order.push(e);
        }
    }
}

/// Emits one array element type's `$array_to_string_t<id>`: renders `[e0, e1, ...]`, converting each
/// element via [`value_to_string_call`]. The array block is `[len: i32][elem0][elem1]...`.
pub(super) fn emit_array_to_string(
    out: &mut String,
    elem: TypeId,
    interner: &TypeInterner,
    strings: &IndexMap<String, u32>,
) {
    let (esize, _) = scalar_size(interner, elem);
    let _ = writeln!(
        out,
        "(func {} (param $ptr i32) (result i32)",
        array_to_string_sym(elem)
    );
    out.push_str("  (local $res i32)\n  (local $len i32)\n  (local $i i32)\n");
    let _ = writeln!(out, "  (i32.const {}) (local.set $res)", strings["["]);
    out.push_str("  (local.get $ptr) (i32.load) (local.set $len)\n");
    out.push_str("  (i32.const 0) (local.set $i)\n");
    out.push_str("  (block $done (loop $scan\n");
    out.push_str("    (local.get $i) (local.get $len) (i32.ge_s) (br_if $done)\n");
    let _ = writeln!(
        out,
        "    (local.get $i) (i32.const 0) (i32.gt_s) (if (then (local.get $res) (i32.const {}) (call $concat_strings) (local.set $res)))",
        strings[", "]
    );
    out.push_str("    (local.get $res)\n    (local.get $ptr) (i32.const 4) (i32.add)\n");
    if esize == 1 {
        out.push_str("    (local.get $i) (i32.add)\n");
    } else {
        let _ = writeln!(
            out,
            "    (local.get $i) (i32.const {}) (i32.mul) (i32.add)",
            esize
        );
    }
    let _ = writeln!(out, "    ({})", load_instr_for(interner, elem));
    if let Some(call) = value_to_string_call(interner, elem) {
        let _ = writeln!(out, "    (call {})", call);
    }
    out.push_str("    (call $concat_strings) (local.set $res)\n");
    out.push_str("    (local.get $i) (i32.const 1) (i32.add) (local.set $i)\n");
    out.push_str("    (br $scan)))\n");
    let _ = writeln!(
        out,
        "  (local.get $res) (i32.const {}) (call $concat_strings)",
        strings["]"]
    );
    out.push_str(")\n");
}

/// Emits `$object_to_string`: null → `"null"`, boxed primitives → unbox + `*_to_string`, strings →
/// identity, each struct/union tag → its `$<Type>_to_string`, everything else → `"<object>"`.
pub(super) fn emit_object_to_string(
    out: &mut String,
    mir: &crate::mir::Mir,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
) {
    out.push_str("(func $object_to_string (param $ptr i32) (result i32)\n  (local $tag i32)\n");
    let _ = writeln!(
        out,
        "  (local.get $ptr) (i32.eqz) (if (then (i32.const {}) (return)))",
        strings["null"]
    );
    out.push_str("  (local.get $ptr) (call $object_tag) (local.set $tag)\n");
    // Boxed primitives: unbox then format; `string` is already its own pointer.
    for e in PRIM_TABLE {
        let body = match (e.unbox_fn, e.to_string) {
            (Some(unbox), Some(to_str)) => {
                format!("(local.get $ptr) (call {}) (call {})", unbox, to_str)
            }
            _ => "(local.get $ptr)".to_string(),
        };
        write_tag_arm(out, e.tag, &body);
    }
    write_struct_union_tag_arms(out, mir, tags, |name| {
        format!("(local.get $ptr) (call ${}_to_string)", name)
    });
    let _ = writeln!(out, "  (i32.const {})\n)", strings["<object>"]);
}

/// Writes one `if (tag == n) {{ <body>; return }}` dispatch arm (matching the `$tag` local set by the
/// tag-dispatch prologue).
pub(super) fn write_tag_arm(out: &mut String, tag: i32, body: &str) {
    let _ = writeln!(
        out,
        "  (local.get $tag) (i32.const {}) (i32.eq) (if (then {} (return)))",
        tag, body
    );
}

/// Writes one tag-dispatch arm per user struct, then per user union (in `tags`-assigned order),
/// calling `body(name)` for the arm body of the type named `name`. Shared by the tag-dispatching
/// routers (`$object_to_string`, `$object_hash_code`, `$release_object`), which agree on the
/// per-type arm structure and differ only in the `$<Type>_*` helper each invokes.
pub(super) fn write_struct_union_tag_arms(
    out: &mut String,
    mir: &crate::mir::Mir,
    tags: &HashMap<TypeId, i32>,
    body: impl Fn(&str) -> String,
) {
    for (ty, layout) in &mir.layouts.structs {
        if let Some(&tag) = tags.get(ty) {
            write_tag_arm(out, tag, &body(&layout.name));
        }
    }
    for (ty, layout) in &mir.layouts.unions {
        if let Some(&tag) = tags.get(ty) {
            write_tag_arm(out, tag, &body(&layout.name));
        }
    }
}
