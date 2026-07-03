use super::*;

/// The WASM value type for a Dream type (`i32`/`i64`/`f32`/`f64`), used for global declarations.
pub(crate) fn wasm_ty_of(interner: &TypeInterner, ty: TypeId) -> &'static str {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Double) => "f64",
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64",
        TyKind::Prim(PrimTy::Float) => "f32",
        _ => "i32",
    }
}

pub(super) fn zero_literal(wasm_ty: &str) -> &'static str {
    match wasm_ty {
        "f64" => "(f64.const 0)",
        "f32" => "(f32.const 0)",
        "i64" => "(i64.const 0)",
        _ => "(i32.const 0)",
    }
}

/// The load instruction for a value of `ty` (width-aware; sub-word scalars zero-extend). Free
/// counterpart of [`Emitter::load_instr`], used by the generated object-protocol helpers.
pub(super) fn load_instr_for(interner: &TypeInterner, ty: TypeId) -> &'static str {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Float) => "f32.load",
        TyKind::Prim(PrimTy::Double) => "f64.load",
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64.load",
        TyKind::Prim(PrimTy::Bool | PrimTy::Char | PrimTy::Byte) => "i32.load8_u",
        _ => "i32.load",
    }
}

/// The store instruction matching [`load_instr_for`] (width-aware; sub-word scalars truncate). Used
/// by the generated struct<->js marshalers to write a field/element slot of type `ty`.
pub(super) fn store_instr_for(interner: &TypeInterner, ty: TypeId) -> &'static str {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Float) => "f32.store",
        TyKind::Prim(PrimTy::Double) => "f64.store",
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64.store",
        TyKind::Prim(PrimTy::Bool | PrimTy::Char | PrimTy::Byte) => "i32.store8",
        _ => "i32.store",
    }
}

/// The `$*_to_string` call that turns a loaded value of `ty` into a string pointer, or `None` when
/// the value already *is* a string pointer (`string`, needing no conversion). Enums render as their
/// The primitive kind of `ty` (stripping nullability), or `None` for reference/`object`/other types.
pub(super) fn prim_of(interner: &TypeInterner, ty: TypeId) -> Option<PrimTy> {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(p) => Some(*p),
        _ => None,
    }
}

/// The `$box_*` runtime helper for boxing primitive `p` into an `object`; `None` for non-boxable
/// (reference) primitives like `string` (already a pointer).
pub(super) fn box_fn_for(p: PrimTy) -> Option<&'static str> {
    Some(match p {
        PrimTy::Int => "$box_int",
        PrimTy::Float => "$box_float",
        PrimTy::Double => "$box_double",
        PrimTy::Bool => "$box_bool",
        PrimTy::Char => "$box_char",
        PrimTy::Long => "$box_long",
        PrimTy::ULong => "$box_ulong",
        PrimTy::UInt => "$box_uint",
        PrimTy::Byte => "$box_byte",
        PrimTy::String => return None,
    })
}

/// The `$unbox_*` runtime helper matching [`box_fn_for`].
pub(super) fn unbox_fn_for(p: PrimTy) -> Option<&'static str> {
    Some(match p {
        PrimTy::Int => "$unbox_int",
        PrimTy::Float => "$unbox_float",
        PrimTy::Double => "$unbox_double",
        PrimTy::Bool => "$unbox_bool",
        PrimTy::Char => "$unbox_char",
        PrimTy::Long => "$unbox_long",
        PrimTy::ULong => "$unbox_ulong",
        PrimTy::UInt => "$unbox_uint",
        PrimTy::Byte => "$unbox_byte",
        PrimTy::String => return None,
    })
}

/// The runtime tag a value of type `ty` carries when boxed as an `object`: a fixed constant for
/// primitives/string, or the struct/union's assigned tag (from `tags`). Used to lower a runtime
/// `x is T` test to an `$object_tag` comparison.
pub(super) fn runtime_tag_for(interner: &TypeInterner, tags: &HashMap<TypeId, i32>, ty: TypeId) -> Option<i32> {
    use crate::mir::abi as t;
    let stripped = interner.strip_nullable(ty);
    match interner.kind(stripped) {
        TyKind::Prim(PrimTy::Int) => Some(t::TAG_INT),
        TyKind::Prim(PrimTy::Float) => Some(t::TAG_FLOAT),
        TyKind::Prim(PrimTy::Double) => Some(t::TAG_DOUBLE),
        TyKind::Prim(PrimTy::Bool) => Some(t::TAG_BOOL),
        TyKind::Prim(PrimTy::String) => Some(t::TAG_STRING),
        TyKind::Prim(PrimTy::Char) => Some(t::TAG_CHAR),
        TyKind::Prim(PrimTy::Long) => Some(t::TAG_LONG),
        TyKind::Prim(PrimTy::UInt) => Some(t::TAG_UINT),
        TyKind::Prim(PrimTy::ULong) => Some(t::TAG_ULONG),
        TyKind::Prim(PrimTy::Byte) => Some(t::TAG_BYTE),
        TyKind::Array(_) => Some(t::TAG_ARRAY),
        _ => tags.get(&stripped).copied(),
    }
}

/// `int` value; arrays dispatch to their element-typed `$array_to_string_t<id>` (arrays are not
/// self-describing at runtime, so the call is chosen statically); other reference types route through
/// the tag-dispatching `$object_to_string`.
pub(super) fn value_to_string_call(interner: &TypeInterner, ty: TypeId) -> Option<String> {
    let call = match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Int) => "$int_to_string",
        TyKind::Prim(PrimTy::Bool) => "$bool_to_string",
        TyKind::Prim(PrimTy::Char) => "$char_to_string",
        TyKind::Prim(PrimTy::Float) => "$float_to_string",
        TyKind::Prim(PrimTy::Double) => "$double_to_string",
        TyKind::Prim(PrimTy::Long) => "$long_to_string",
        TyKind::Prim(PrimTy::ULong) => "$ulong_to_string",
        TyKind::Prim(PrimTy::UInt) => "$uint_to_string",
        TyKind::Prim(PrimTy::Byte) => "$byte_to_string",
        TyKind::Prim(PrimTy::String) => return None,
        TyKind::Enum(_) => "$int_to_string",
        TyKind::Array(elem) => return Some(array_to_string_sym(*elem)),
        _ => "$object_to_string",
    };
    Some(call.to_string())
}

/// The symbol of the generated element-typed array `to_string` helper for element type `elem`.
pub(super) fn array_to_string_sym(elem: TypeId) -> String {
    format!("$array_to_string_t{}", elem.0)
}

/// Maps a callee symbol to an async-intrinsic kind (`sleep`, `__promise_all`, …), if any.
pub(super) fn async_intrinsic_kind(sym: &str) -> Option<&'static str> {
    use crate::intrinsics;
    // Intrinsics are keyed by their `@intrinsic("…")` attribute string in the symbol table (e.g.
    // `promise_all`), so match those here as well as the internal `__promise_*` names.
    if sym.ends_with("_sleep") || sym == intrinsics::SLEEP {
        Some(intrinsics::SLEEP)
    } else if sym == intrinsics::PROMISE_ALL || sym == intrinsics::ATTR_PROMISE_ALL {
        Some(intrinsics::PROMISE_ALL)
    } else if sym == intrinsics::PROMISE_ANY
        || sym == intrinsics::PROMISE_RACE
        || sym == intrinsics::ATTR_PROMISE_ANY
        || sym == intrinsics::ATTR_PROMISE_RACE
    {
        Some(intrinsics::PROMISE_ANY)
    } else {
        None
    }
}