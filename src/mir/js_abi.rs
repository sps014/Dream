//! The JavaScript-interop ABI, isolated from the general compiler.
//!
//! Everything the backend and the prune pass must agree on about *how* Dream talks to the JS host —
//! and which is specific to `js` interop rather than to any one compiler stage — lives here so there
//! is a single source of truth: the tagged argument-slot layout used by dynamic `js` calls
//! ([`Emitter::emit_js_call`](crate::mir::emit)), the symbol names of the generated struct/array
//! marshalers ([`mir::emit::js_marshal`](crate::mir::emit)), and the host bridge set those marshalers
//! call ([`marshal_bridge_defs`], consumed by [`prune`](crate::mir::prune)).

use crate::mir::Mir;
use crate::types::{method_fn, DefId, PrimTy, TyKind, TypeId, TypeInterner};

/// The host module every `@js` bridge is imported from: matches the first argument of the
/// `@js("Dream", …)` attributes in `stdlib/core/js.dream` and the module object installed by
/// `runtime/dream.js`.
pub(crate) const HOST_MODULE: &str = "Dream";

/// The Dream type name whose stdlib methods back every interop bridge. Combined with a method name
/// via [`method_fn`] it yields the mangled symbol an `@js` extern is emitted/imported under.
const JS_TYPE: &str = "js";

/// The WAT symbol of the `js.<method>` stdlib bridge (e.g. `bridge_sym("__box_int")` -> `$js___box_int`),
/// derived through the one canonical mangler so the generated marshalers never hard-code the scheme.
pub(crate) fn bridge_sym(method: &str) -> String {
    format!("${}", method_fn(JS_TYPE, method))
}

/// The `js` stdlib methods the generated marshalers (`emit::js_marshal`) call. Single source of
/// truth: the marshaler WAT references each via [`bridge_sym`], and [`marshal_bridge_defs`] keeps the
/// matching imports from being tree-shaken when a struct<->js cast survives.
const MARSHAL_METHODS: [&str; 16] = [
    "object",
    "array",
    "__set",
    "__get",
    "__index_set",
    "__index_get",
    "__box_int",
    "__box_long",
    "__box_double",
    "__box_bool",
    "__box_string",
    "__as_int",
    "__as_long",
    "__as_double",
    "__as_bool",
    "__as_string",
];

/// The `DefId`s of the bridge imports the generated struct/array marshalers depend on. The marshalers
/// are raw WAT (no MIR call edge references them), so [`prune`](crate::mir::prune) uses this to keep
/// their imports alive whenever a struct<->js cast survives. Matches each import's mangled `name`
/// against the [`MARSHAL_METHODS`] set via the shared [`method_fn`] mangler — no ad-hoc name parsing.
pub(crate) fn marshal_bridge_defs(mir: &Mir) -> Vec<DefId> {
    let wanted: Vec<String> = MARSHAL_METHODS.iter().map(|m| method_fn(JS_TYPE, m)).collect();
    mir.imports
        .iter()
        .filter(|imp| imp.module == HOST_MODULE && wanted.contains(&imp.name))
        .map(|imp| imp.def)
        .collect()
}

// -- Generated-marshaler symbol names ------------------------------------------------------------

/// `$<Name>_to_js`: the marshaler that deep-copies a struct/class into a plain JS object.
pub(crate) fn struct_to_js_sym(name: &str) -> String {
    format!("${}_to_js", name)
}
/// `$js_to_<Name>`: the marshaler that rebuilds a struct/class from a JS object's properties.
pub(crate) fn js_to_struct_sym(name: &str) -> String {
    format!("$js_to_{}", name)
}
/// `$array_to_js_t<id>`: the marshaler that copies a Dream `elem[]` into a JS array.
pub(crate) fn array_to_js_sym(elem: TypeId) -> String {
    format!("$array_to_js_t{}", elem.0)
}
/// `$js_to_array_t<id>`: the marshaler that copies a JS array into a fresh Dream `elem[]`.
pub(crate) fn js_to_array_sym(elem: TypeId) -> String {
    format!("$js_to_array_t{}", elem.0)
}

// -- Dynamic-call argument slots -----------------------------------------------------------------

/// Bytes per argument slot in the dynamic-`js`-call buffer, laid out as
/// `[tag: i32 @ +0][aux: i32 @ +4][payload: 8 bytes @ +8]`.
pub(crate) const SLOT_SIZE: u32 = 16;
/// Byte offset of a slot's `aux` word (see [`slot_desc`]).
pub(crate) const SLOT_AUX_OFFSET: u32 = 4;
/// Byte offset of a slot's 8-byte payload.
pub(crate) const SLOT_PAYLOAD_OFFSET: u32 = 8;

/// Slot tags identifying how the host decodes a slot's payload (see the `decodeJsSlots` decoder in
/// `runtime/dream.js`).
pub(crate) mod tag {
    /// Part of the wire ABI (the host decoder maps it to `null`); the emitter never writes it today
    /// because a `null` argument is rejected earlier, so it is intentionally unused on this side.
    #[allow(dead_code)]
    pub const NULL: i32 = 0;
    pub const INT: i32 = 1;
    pub const LONG: i32 = 2;
    pub const DOUBLE: i32 = 3;
    pub const BOOL: i32 = 4;
    pub const STRING: i32 = 5;
    pub const JS: i32 = 6;
    pub const FUNC: i32 = 7;
    pub const ARRAY: i32 = 8;
}

/// How a `js`-call argument of type `ty` is written into its 16-byte slot: `(tag, aux, payload store
/// instruction)`. `aux` carries the element tag for an `ARRAY` slot and the parameter count for a
/// `FUNC` slot (so the host wraps the funcref with the right arity); it is `0` otherwise. The payload
/// store is `i64.store`/`f64.store` for wide scalars, else `i32.store`.
pub(crate) fn slot_desc(interner: &TypeInterner, ty: TypeId) -> (i32, i32, &'static str) {
    let stripped = interner.strip_nullable(ty);
    match interner.kind(stripped) {
        TyKind::Js => (tag::JS, 0, "i32.store"),
        TyKind::Enum(_) => (tag::INT, 0, "i32.store"),
        TyKind::Func(params, _) => (tag::FUNC, params.len() as i32, "i32.store"),
        TyKind::Array(elem) => (tag::ARRAY, slot_tag(interner, *elem), "i32.store"),
        TyKind::Prim(p) => match p {
            PrimTy::String => (tag::STRING, 0, "i32.store"),
            PrimTy::Bool => (tag::BOOL, 0, "i32.store"),
            PrimTy::Double | PrimTy::Float => (tag::DOUBLE, 0, "f64.store"),
            PrimTy::Long | PrimTy::ULong => (tag::LONG, 0, "i64.store"),
            PrimTy::Int | PrimTy::UInt | PrimTy::Byte | PrimTy::Char => (tag::INT, 0, "i32.store"),
        },
        // The analyzer rejects other types as `js` arguments; treat any leftover as a handle.
        _ => (tag::JS, 0, "i32.store"),
    }
}

/// The bare slot tag of `ty` (no payload/store info), used as the `aux` element tag of an `ARRAY`
/// slot so the host decodes the array's elements with the right width/kind.
fn slot_tag(interner: &TypeInterner, ty: TypeId) -> i32 {
    slot_desc(interner, ty).0
}
