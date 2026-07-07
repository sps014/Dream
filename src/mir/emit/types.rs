use super::*;

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
    prim_info(p).box_fn
}

/// The `$unbox_*` runtime helper matching [`box_fn_for`].
pub(super) fn unbox_fn_for(p: PrimTy) -> Option<&'static str> {
    prim_info(p).unbox_fn
}

/// The runtime tag a value of type `ty` carries when boxed as an `object`: a fixed constant for
/// primitives/string, or the struct/union's assigned tag (from `tags`). Used to lower a runtime
/// `x is T` test to an `$object_tag` comparison.
pub(super) fn runtime_tag_for(
    interner: &TypeInterner,
    tags: &HashMap<TypeId, i32>,
    ty: TypeId,
) -> Option<i32> {
    let stripped = interner.strip_nullable(ty);
    match interner.kind(stripped) {
        TyKind::Prim(p) => Some(prim_info(*p).tag),
        TyKind::Array(_) => Some(crate::mir::abi::TAG_ARRAY),
        _ => tags.get(&stripped).copied(),
    }
}

/// The `$*_to_string` call that turns a loaded value of `ty` into a string pointer, or `None` when
/// the value already *is* a string pointer (`string`, needing no conversion). Enums render as their
/// `int` value; arrays dispatch to their element-typed `$array_to_string_t<id>` (arrays are not
/// self-describing at runtime, so the call is chosen statically); other reference types route through
/// the tag-dispatching `$object_to_string`.
pub(super) fn value_to_string_call(interner: &TypeInterner, ty: TypeId) -> Option<String> {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(p) => prim_info(*p).to_string.map(|s| s.to_string()),
        TyKind::Enum(_) => Some("$int_to_string".to_string()),
        TyKind::Array(elem) => Some(array_to_string_sym(*elem)),
        _ => Some("$object_to_string".to_string()),
    }
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
