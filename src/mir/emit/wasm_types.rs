//! The single source of truth for mapping a Dream [`TypeId`] to its WebAssembly representation:
//! the value type (`i32`/`i64`/`f32`/`f64`) and the width-aware load/store instructions. Both the
//! `Emitter` (which builds `String`s incrementally) and the string-generating helpers
//! (`protocol`, `js_marshal`, `module`, async lowering) route through these free functions so the
//! primitive-width rules are defined exactly once.

use super::*;

/// The WASM value type for a Dream type (`i32`/`i64`/`f32`/`f64`). Nullability is stripped first;
/// every reference/`object`/`void` type is an `i32` pointer/word.
pub(crate) fn wasm_ty_of(interner: &TypeInterner, ty: TypeId) -> &'static str {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Double) => "f64",
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64",
        TyKind::Prim(PrimTy::Float) => "f32",
        _ => "i32",
    }
}

/// The zero constant of a WASM value type, used to default-initialize a slot of that type.
pub(super) fn zero_literal(wasm_ty: &str) -> &'static str {
    match wasm_ty {
        "f64" => "(f64.const 0)",
        "f32" => "(f32.const 0)",
        "i64" => "(i64.const 0)",
        _ => "(i32.const 0)",
    }
}

/// The load instruction for a value of `ty` (width-aware; sub-word scalars zero-extend).
pub(super) fn load_instr_for(interner: &TypeInterner, ty: TypeId) -> &'static str {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Float) => "f32.load",
        TyKind::Prim(PrimTy::Double) => "f64.load",
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64.load",
        TyKind::Prim(PrimTy::Bool | PrimTy::Char | PrimTy::Byte) => "i32.load8_u",
        _ => "i32.load",
    }
}

/// The store instruction matching [`load_instr_for`] (width-aware; sub-word scalars truncate).
pub(super) fn store_instr_for(interner: &TypeInterner, ty: TypeId) -> &'static str {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Float) => "f32.store",
        TyKind::Prim(PrimTy::Double) => "f64.store",
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64.store",
        TyKind::Prim(PrimTy::Bool | PrimTy::Char | PrimTy::Byte) => "i32.store8",
        _ => "i32.store",
    }
}

/// The per-primitive runtime dispatch descriptor: the object-header `tag` a boxed value carries, the
/// `$box_*`/`$unbox_*` runtime helpers, the `$*_to_string` formatter, and the instructions that turn
/// a loaded value into its `i32` hash. This is the single source of truth for every place codegen
/// dispatches on a primitive: `value_to_string_call`, `box_fn_for`/`unbox_fn_for`, `runtime_tag_for`
/// (in `types`), the `$object_to_string`/`$object_hash_code` routers (in `protocol`), and the `print`
/// statement (in `emitter::statements`).
///
/// `string` is a heap reference (its own pointer), so it has no box/unbox/`to_string` helper (`None`)
/// and hashes via `$hash_string`.
pub(super) struct PrimInfo {
    pub(super) prim: PrimTy,
    pub(super) tag: i32,
    pub(super) box_fn: Option<&'static str>,
    pub(super) unbox_fn: Option<&'static str>,
    pub(super) to_string: Option<&'static str>,
    pub(super) hash: &'static str,
}

/// Every primitive's runtime descriptor. The order here is load-bearing: the object-protocol routers
/// iterate this table to emit their tag-dispatch arms, so it defines the (stable) arm order in the
/// generated `$object_to_string`/`$object_hash_code`.
pub(super) const PRIM_TABLE: &[PrimInfo] = {
    use crate::mir::abi as t;
    &[
        PrimInfo {
            prim: PrimTy::Int,
            tag: t::TAG_INT,
            box_fn: Some("$box_int"),
            unbox_fn: Some("$unbox_int"),
            to_string: Some("$int_to_string"),
            hash: "",
        },
        PrimInfo {
            prim: PrimTy::Float,
            tag: t::TAG_FLOAT,
            box_fn: Some("$box_float"),
            unbox_fn: Some("$unbox_float"),
            to_string: Some("$float_to_string"),
            hash: "(i32.reinterpret_f32)",
        },
        PrimInfo {
            prim: PrimTy::Double,
            tag: t::TAG_DOUBLE,
            box_fn: Some("$box_double"),
            unbox_fn: Some("$unbox_double"),
            to_string: Some("$double_to_string"),
            hash: "(call $hash_double)",
        },
        PrimInfo {
            prim: PrimTy::Bool,
            tag: t::TAG_BOOL,
            box_fn: Some("$box_bool"),
            unbox_fn: Some("$unbox_bool"),
            to_string: Some("$bool_to_string"),
            hash: "",
        },
        PrimInfo {
            prim: PrimTy::Char,
            tag: t::TAG_CHAR,
            box_fn: Some("$box_char"),
            unbox_fn: Some("$unbox_char"),
            to_string: Some("$char_to_string"),
            hash: "",
        },
        PrimInfo {
            prim: PrimTy::Long,
            tag: t::TAG_LONG,
            box_fn: Some("$box_long"),
            unbox_fn: Some("$unbox_long"),
            to_string: Some("$long_to_string"),
            hash: "(call $hash_long)",
        },
        PrimInfo {
            prim: PrimTy::ULong,
            tag: t::TAG_ULONG,
            box_fn: Some("$box_ulong"),
            unbox_fn: Some("$unbox_ulong"),
            to_string: Some("$ulong_to_string"),
            hash: "(call $hash_long)",
        },
        PrimInfo {
            prim: PrimTy::UInt,
            tag: t::TAG_UINT,
            box_fn: Some("$box_uint"),
            unbox_fn: Some("$unbox_uint"),
            to_string: Some("$uint_to_string"),
            hash: "",
        },
        PrimInfo {
            prim: PrimTy::Byte,
            tag: t::TAG_BYTE,
            box_fn: Some("$box_byte"),
            unbox_fn: Some("$unbox_byte"),
            to_string: Some("$byte_to_string"),
            hash: "",
        },
        PrimInfo {
            prim: PrimTy::String,
            tag: t::TAG_STRING,
            box_fn: None,
            unbox_fn: None,
            to_string: None,
            hash: "(call $hash_string)",
        },
    ]
};

/// The [`PrimInfo`] descriptor for primitive `p`.
pub(super) fn prim_info(p: PrimTy) -> &'static PrimInfo {
    PRIM_TABLE
        .iter()
        .find(|e| e.prim == p)
        .expect("every PrimTy has a PRIM_TABLE entry")
}
