use super::*;
use crate::mir::js_abi::{
    array_to_js_sym, bridge_sym, js_to_array_sym, js_to_struct_sym, struct_to_js_sym,
};

/// The generated marshaler [`Emitter::emit_cast`] must call for a `Cast` between `from` and `to`
/// when exactly one side is `js` and the other has a struct/class layout: `$<Name>_to_js` for a
/// Dream->js cast, `$js_to_<Name>` for js->Dream. `None` for any cast that is not struct<->js (the
/// caller falls back to the ordinary primitive box/unbox path).
pub(super) fn cast_sym(
    interner: &TypeInterner,
    layouts: &LayoutTable,
    from: TypeId,
    to: TypeId,
) -> Option<String> {
    let from_s = interner.strip_nullable(from);
    let to_s = interner.strip_nullable(to);
    let is_js = |t: TypeId| matches!(interner.kind(t), TyKind::Js);
    if is_js(to_s) {
        return layouts.get(from_s).map(|l| struct_to_js_sym(&l.name));
    }
    if is_js(from_s) {
        return layouts.get(to_s).map(|l| js_to_struct_sym(&l.name));
    }
    None
}

/// Emits the generated struct/class <-> JS object marshalers that back a `Cast` between a
/// struct/class type and `js` (wired up in [`Emitter::emit_cast`]). For every struct/class layout
/// this emits `$<Name>_to_js` (build a plain JS object, deep-copying each field) and `$js_to_<Name>`
/// (allocate the object and populate each field from the JS object's properties). Array-typed fields
/// route through per-element-type `$array_to_js_t<id>` / `$js_to_array_t<id>` helpers. Fields whose
/// type is not marshalable (maps, interfaces, inline value structs, ...) are skipped on the way out
/// and zeroed on the way in.
///
/// Every struct gets both helpers; the whole-module WAT DCE drops the ones no `Cast` references, and
/// [`prune_dead_imports`](crate::mir::prune) keeps the `js*` host bridges alive whenever a
/// struct<->js cast survives.
pub(super) fn emit_js_marshal(
    out: &mut String,
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
) {
    for (ty, layout) in &mir.layouts.structs {
        emit_struct_to_js(out, layout, interner, mir, strings);
        emit_js_to_struct(out, *ty, layout, interner, mir, strings, tags);
    }
    for elem in array_elem_types(mir, interner) {
        if is_marshalable(interner, elem) {
            emit_array_to_js(out, elem, interner, mir);
            emit_js_to_array(out, elem, interner, mir, strings);
        }
    }
}

/// Whether a value of `ty` can cross the struct<->js boundary as a field/element: primitives, enums,
/// `string`, `js`, reference struct/class types, and arrays of the same. Inline value structs, maps,
/// interfaces, and functions are not marshalable here.
fn is_marshalable(interner: &TypeInterner, ty: TypeId) -> bool {
    let s = interner.strip_nullable(ty);
    match interner.kind(s) {
        TyKind::Prim(_) | TyKind::Enum(_) | TyKind::Js => true,
        TyKind::Array(elem) => is_marshalable(interner, *elem),
        TyKind::Struct(..) => interner.is_reference(s),
        _ => false,
    }
}

/// The `$<Name>` of the struct/class `ty` names, or `None` if it has no struct layout.
fn struct_name(mir: &crate::mir::Mir, ty: TypeId) -> Option<String> {
    mir.layouts.structs.get(&ty).map(|l| l.name.clone())
}

/// `(preconvert, box_method)` turning a loaded primitive of kind `p` into a `js` handle. The method
/// name resolves to a bridge symbol via [`bridge_sym`]; `preconvert` widens a `float` to the `f64`
/// the `double` box expects.
fn box_prim(p: PrimTy) -> (&'static str, &'static str) {
    match p {
        PrimTy::Int | PrimTy::UInt | PrimTy::Byte | PrimTy::Char => ("", "__box_int"),
        PrimTy::Long | PrimTy::ULong => ("", "__box_long"),
        PrimTy::Float => ("(f64.promote_f32)", "__box_double"),
        PrimTy::Double => ("", "__box_double"),
        PrimTy::Bool => ("", "__box_bool"),
        PrimTy::String => ("", "__box_string"),
    }
}

/// `(unbox_method, postconvert)` turning a `js` handle into a value of primitive kind `p`. The method
/// name resolves to a bridge symbol via [`bridge_sym`]; `postconvert` narrows the `f64` an unboxed
/// `double` yields back to a `float`.
fn unbox_prim(p: PrimTy) -> (&'static str, &'static str) {
    match p {
        PrimTy::Int | PrimTy::UInt | PrimTy::Byte | PrimTy::Char => ("__as_int", ""),
        PrimTy::Long | PrimTy::ULong => ("__as_long", ""),
        PrimTy::Float => ("__as_double", "(f32.demote_f64)"),
        PrimTy::Double => ("__as_double", ""),
        PrimTy::Bool => ("__as_bool", ""),
        PrimTy::String => ("__as_string", ""),
    }
}

/// WAT pushing the address of a slot at `base + off` (`base` is a WAT snippet pushing the base ptr).
fn addr_at(base: &str, off: u32) -> String {
    if off == 0 {
        base.to_string()
    } else {
        format!("{base} (i32.const {off}) (i32.add)")
    }
}

/// WAT that consumes nothing and pushes a `js` handle for the value of type `ty` living at `addr`
/// (a WAT snippet pushing the value's *address*). `None` when `ty` is not marshalable.
fn value_to_js(
    interner: &TypeInterner,
    mir: &crate::mir::Mir,
    addr: &str,
    ty: TypeId,
) -> Option<String> {
    let s = interner.strip_nullable(ty);
    let load = load_instr_for(interner, ty);
    match interner.kind(s) {
        TyKind::Prim(p) => {
            let (pre, boxm) = box_prim(*p);
            Some(format!("{addr} ({load}) {pre} (call {})", bridge_sym(boxm)))
        }
        TyKind::Enum(_) => Some(format!(
            "{addr} ({load}) (call {})",
            bridge_sym("__box_int")
        )),
        TyKind::Js => Some(format!("{addr} ({load})")),
        TyKind::Array(elem) if is_marshalable(interner, *elem) => {
            Some(format!("{addr} ({load}) (call {})", array_to_js_sym(*elem)))
        }
        TyKind::Struct(..) if interner.is_reference(s) => {
            let name = struct_name(mir, s)?;
            Some(format!(
                "{addr} ({load}) (call {})",
                struct_to_js_sym(&name)
            ))
        }
        // A nested *value* struct is stored inline: pass its address (no load) to the marshaler.
        TyKind::Struct(..) => {
            let name = struct_name(mir, s)?;
            Some(format!("{addr} (call {})", struct_to_js_sym(&name)))
        }
        _ => None,
    }
}

/// WAT that consumes nothing and pushes the Dream value of type `ty` decoded from the `js` handle
/// produced by `jsval` (a WAT snippet pushing that handle). `None` when `ty` is not marshalable.
fn value_from_js(
    interner: &TypeInterner,
    mir: &crate::mir::Mir,
    jsval: &str,
    ty: TypeId,
) -> Option<String> {
    let s = interner.strip_nullable(ty);
    match interner.kind(s) {
        TyKind::Prim(p) => {
            let (unbox, post) = unbox_prim(*p);
            Some(format!("{jsval} (call {}) {post}", bridge_sym(unbox)))
        }
        TyKind::Enum(_) => Some(format!("{jsval} (call {})", bridge_sym("__as_int"))),
        TyKind::Js => Some(jsval.to_string()),
        TyKind::Array(elem) if is_marshalable(interner, *elem) => {
            Some(format!("{jsval} (call {})", js_to_array_sym(*elem)))
        }
        TyKind::Struct(..) if interner.is_reference(s) => {
            let name = struct_name(mir, s)?;
            Some(format!("{jsval} (call {})", js_to_struct_sym(&name)))
        }
        _ => None,
    }
}

/// `$<Name>_to_js`: `{}` -> set each marshalable field as a property.
fn emit_struct_to_js(
    out: &mut String,
    layout: &crate::hir::TypeLayout,
    interner: &TypeInterner,
    mir: &crate::mir::Mir,
    strings: &IndexMap<String, u32>,
) {
    let _ = writeln!(
        out,
        "(func {} (param $this i32) (result i32)",
        struct_to_js_sym(&layout.name)
    );
    out.push_str("  (local $o i32)\n");
    let _ = writeln!(out, "  (call {}) (local.set $o)", bridge_sym("object"));
    for f in &layout.fields {
        let addr = addr_at("(local.get $this)", f.offset);
        if let Some(val) = value_to_js(interner, mir, &addr, f.ty) {
            let _ = writeln!(
                out,
                "  (local.get $o) (i32.const {}) {} (call {})",
                strings[&f.name],
                val,
                bridge_sym("__set")
            );
        }
    }
    out.push_str("  (local.get $o)\n)\n");
}

/// `$js_to_<Name>`: allocate the object, then read each marshalable field from a JS property (unset
/// fields are zeroed so reference slots stay null-safe for release).
fn emit_js_to_struct(
    out: &mut String,
    ty: TypeId,
    layout: &crate::hir::TypeLayout,
    interner: &TypeInterner,
    mir: &crate::mir::Mir,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
) {
    let tag = tags.get(&ty).copied().unwrap_or(0);
    let _ = writeln!(
        out,
        "(func {} (param $j i32) (result i32)",
        js_to_struct_sym(&layout.name)
    );
    out.push_str("  (local $o i32)\n");
    let _ = writeln!(
        out,
        "  (i32.const {}) (i32.const {}) (call $malloc) (local.set $o)",
        layout.size, tag
    );
    for f in &layout.fields {
        let dst = addr_at("(local.get $o)", f.offset);
        let jsval = format!(
            "(local.get $j) (i32.const {}) (call {})",
            strings[&f.name],
            bridge_sym("__get")
        );
        if let Some(val) = value_from_js(interner, mir, &jsval, f.ty) {
            let store = store_instr_for(interner, f.ty);
            let _ = writeln!(out, "  {dst} {val} ({store})");
        } else {
            // Skipped field: zero its whole footprint (memory.fill: dst, value, len).
            let (size, _) = scalar_size(interner, f.ty);
            let _ = writeln!(
                out,
                "  {dst} (i32.const 0) (i32.const {size}) (memory.fill)"
            );
        }
    }
    out.push_str("  (local.get $o)\n)\n");
}

/// `$array_to_js_t<id>`: a Dream `elem[]` -> a JS array, deep-copying each element.
fn emit_array_to_js(
    out: &mut String,
    elem: TypeId,
    interner: &TypeInterner,
    mir: &crate::mir::Mir,
) {
    let (esize, _) = scalar_size(interner, elem);
    let addr = format!(
        "(local.get $arr) (i32.const 4) (i32.add) (local.get $i) (i32.const {esize}) (i32.mul) (i32.add)"
    );
    let val = value_to_js(interner, mir, &addr, elem).expect("array element is marshalable");
    let _ = writeln!(
        out,
        "(func {} (param $arr i32) (result i32)",
        array_to_js_sym(elem)
    );
    out.push_str("  (local $o i32) (local $i i32) (local $n i32)\n");
    let _ = writeln!(out, "  (call {}) (local.set $o)", bridge_sym("array"));
    out.push_str("  (local.get $arr) (i32.load) (local.set $n)\n");
    out.push_str("  (block $brk (loop $lp\n");
    out.push_str("    (local.get $i) (local.get $n) (i32.ge_s) (br_if $brk)\n");
    let _ = writeln!(
        out,
        "    (local.get $o) (local.get $i) (call {}) {val} (call {})",
        bridge_sym("__box_int"),
        bridge_sym("__index_set")
    );
    out.push_str("    (local.get $i) (i32.const 1) (i32.add) (local.set $i)\n");
    out.push_str("    (br $lp)))\n");
    out.push_str("  (local.get $o)\n)\n");
}

/// `$js_to_array_t<id>`: a JS array (its `length` + indexed elements) -> a fresh Dream `elem[]`.
fn emit_js_to_array(
    out: &mut String,
    elem: TypeId,
    interner: &TypeInterner,
    mir: &crate::mir::Mir,
    strings: &IndexMap<String, u32>,
) {
    let (esize, _) = scalar_size(interner, elem);
    let store = store_instr_for(interner, elem);
    let jsval = format!(
        "(local.get $j) (local.get $i) (call {}) (call {})",
        bridge_sym("__box_int"),
        bridge_sym("__index_get")
    );
    let val = value_from_js(interner, mir, &jsval, elem).expect("array element is marshalable");
    let _ = writeln!(
        out,
        "(func {} (param $j i32) (result i32)",
        js_to_array_sym(elem)
    );
    out.push_str("  (local $o i32) (local $i i32) (local $n i32)\n");
    let _ = writeln!(
        out,
        "  (local.get $j) (i32.const {}) (call {}) (call {}) (local.set $n)",
        strings["length"],
        bridge_sym("__get"),
        bridge_sym("__as_int")
    );
    let _ = writeln!(
        out,
        "  (i32.const 4) (local.get $n) (i32.const {esize}) (i32.mul) (i32.add) (i32.const {}) (call $malloc) (local.set $o)",
        ARRAY_TAG
    );
    out.push_str("  (local.get $o) (local.get $n) (i32.store)\n");
    out.push_str("  (block $brk (loop $lp\n");
    out.push_str("    (local.get $i) (local.get $n) (i32.ge_s) (br_if $brk)\n");
    let dst = format!(
        "(local.get $o) (i32.const 4) (i32.add) (local.get $i) (i32.const {esize}) (i32.mul) (i32.add)"
    );
    let _ = writeln!(out, "    {dst} {val} ({store})");
    out.push_str("    (local.get $i) (i32.const 1) (i32.add) (local.set $i)\n");
    out.push_str("    (br $lp)))\n");
    out.push_str("  (local.get $o)\n)\n");
}
