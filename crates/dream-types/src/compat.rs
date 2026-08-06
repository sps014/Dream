//! Structural type relations over interned [`TypeId`]s: numeric widening and assignability. These
//! replace the string-comparison rules (`compare_data_type`, `type_str_assignable`,
//! `overload_arg_compatible`) with `TypeId`-equality plus explicit widen handling.

use super::{PrimTy, TyKind, TypeId, TypeInterner};

/// True if a value of numeric primitive `from` may implicitly widen to `to` without a cast. Mirrors
/// the legacy `numeric_widen` lattice exactly: `byte -> int -> long -> float -> double`,
/// `byte -> uint -> ulong`, plus the safe unsigned/float cross-edges. Same-width opposite-sign pairs
/// are excluded, and `from == to` is false (identity, handled separately by callers).
pub fn numeric_widen(from: PrimTy, to: PrimTy) -> bool {
    use PrimTy::*;
    matches!(
        (from, to),
        (Byte, Int)
            | (Byte, UInt)
            | (Byte, Long)
            | (Byte, ULong)
            | (Byte, Float)
            | (Byte, Double)
            | (Int, Long)
            | (Int, Float)
            | (Int, Double)
            | (UInt, Long)
            | (UInt, ULong)
            | (UInt, Float)
            | (UInt, Double)
            | (Long, Float)
            | (Long, Double)
            | (ULong, Float)
            | (ULong, Double)
            | (Float, Double)
    )
}

/// True if a value of type `value` may be assigned to a binding/parameter of type `target`. Encodes
/// the same rules as the legacy string checks: `Error` poison is bidirectionally compatible, any
/// value widens into `object`, enums interconvert with `int`, and numeric primitives widen per the
/// lattice.
pub fn assignable(interner: &TypeInterner, target: TypeId, value: TypeId) -> bool {
    if target == value {
        return true;
    }
    let (tk, vk) = (interner.kind(target), interner.kind(value));

    // Poison short-circuits so one error does not cascade.
    if matches!(tk, TyKind::Error) || matches!(vk, TyKind::Error) {
        return true;
    }

    // Everything is assignable to `object`.
    if matches!(tk, TyKind::Object) {
        return true;
    }

    // The dynamic `js` type: any primitive/`string` boxes into `js`, and a `js` value unboxes into
    // any primitive/`string`. The actual box/unbox conversion is materialized by the analyzer's
    // coercion pass; here we only permit the assignment to type-check. `js <-> js` is the identity
    // handled above.
    // A struct/class also deep-copies into a `js` object and reconstructs from one (the backend
    // generates the `$<Type>_to_js` / `$js_to_<Type>` marshalers); reconstruction targets a reference
    // class only.
    if matches!(tk, TyKind::Js) {
        return matches!(vk, TyKind::Prim(_) | TyKind::Struct(..));
    }
    if matches!(vk, TyKind::Js) {
        if matches!(tk, TyKind::Prim(_)) {
            return true;
        }
        if matches!(tk, TyKind::Struct(..)) {
            return interner.is_reference(target);
        }
        return false;
    }

    // Enum <-> int both directions.
    if is_enum_int_pair(tk, vk) {
        return true;
    }

    // Numeric widening.
    if let (TyKind::Prim(from), TyKind::Prim(to)) = (vk, tk) {
        if numeric_widen(*from, *to) {
            return true;
        }
    }

    // Structural tuples: same arity, each element assignable.
    if let (TyKind::Tuple(t_elems), TyKind::Tuple(v_elems)) = (tk, vk) {
        if t_elems.len() == v_elems.len() {
            return t_elems
                .iter()
                .zip(v_elems.iter())
                .all(|(t, v)| assignable(interner, *t, *v));
        }
    }

    false
}

fn is_enum_int_pair(a: &TyKind, b: &TyKind) -> bool {
    matches!(
        (a, b),
        (TyKind::Enum(_), TyKind::Prim(PrimTy::Int)) | (TyKind::Prim(PrimTy::Int), TyKind::Enum(_))
    )
}

/// Overload viability: looser than [`assignable`] — any two numeric primitives are compatible
/// regardless of widening direction (exactness is scored separately by overload ranking). Mirrors
/// the legacy `overload_arg_compatible`.
pub fn overload_compatible(interner: &TypeInterner, param: TypeId, arg: TypeId) -> bool {
    if param == arg {
        return true;
    }
    let (pk, ak) = (interner.kind(param), interner.kind(arg));
    if matches!(pk, TyKind::Error) || matches!(ak, TyKind::Error) {
        return true;
    }
    if matches!(pk, TyKind::Object) {
        return true;
    }
    if is_enum_int_pair(pk, ak) {
        return true;
    }
    if let (TyKind::Prim(p), TyKind::Prim(a)) = (pk, ak) {
        if p.is_numeric() && a.is_numeric() {
            return true;
        }
    }
    false
}
