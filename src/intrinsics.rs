//! Central registry of compiler *intrinsics*: the builtins and namespaced stdlib operations that
//! the compiler recognizes by name and handles specially - typing them in the semantic analyzer
//! and lowering them in the codegen backend - rather than resolving them through the ordinary
//! function/method tables.
//!
//! Every layer (semantic analysis, codegen, and the codegen-side type inference helper)
//! classifies names through the constants and predicates defined here, so the set of recognized
//! intrinsics - and therefore the surface that has to change when one is added, renamed, or
//! removed - lives in exactly one place. Previously these names were hardcoded as bare string
//! literals duplicated across `type_checker.rs`, `expression.rs`, `statement.rs`, `utils.rs`,
//! `async_support.rs`, and `stdlib/mod.rs`.

/// The attribute that tags a stdlib declaration as a compiler intrinsic, e.g.
/// `@intrinsic("print")`. The parser skips emitting a WASM import for it, the function table
/// records its key, and codegen lowers calls to dedicated runtime helpers. Single source of
/// truth for the attribute name across parser/semantics/codegen.
pub const INTRINSIC_ATTR: &str = "intrinsic";

/// Extracts the intrinsic key from a declaration's attribute list, i.e. the `"name"` in
/// `@intrinsic("name")`, or `None` if the declaration is not an intrinsic. Centralizes the
/// attribute lookup + quote-stripping that was previously duplicated across layers.
pub fn intrinsic_key(attributes: &[crate::syntax::nodes::AttributeNode]) -> Option<String> {
    attributes
        .iter()
        .find(|a| a.name.text == INTRINSIC_ATTR)
        .and_then(|a| {
            a.args
                .first()
                .map(|arg| arg.text.trim_matches('"').to_string())
        })
}

/// True if `attributes` contains an `@intrinsic(...)` marker.
pub fn has_intrinsic_attr(attributes: &[crate::syntax::nodes::AttributeNode]) -> bool {
    attributes.iter().any(|a| a.name.text == INTRINSIC_ATTR)
}

// --- Object-protocol builtin free functions -------------------------------------------------
// Callable as `f(x)` with a single argument of any type; lowered to dedicated object-protocol
// runtime helpers rather than to user/stdlib functions.

pub const PRINT: &str = "__print";
pub const PRINTLN: &str = "__println";
/// The object-protocol builtins, surfaced to users as the universal instance methods
/// `x.to_string()` / `x.hash_code()` (see [`TO_STRING`] / [`HASH_CODE`]).
pub const TO_STRING: &str = "to_string";
pub const HASH_CODE: &str = "hash_code";

/// The internal print combinators (lowered from `System.print` / `System.println`). They are not
/// user-callable; `to_string`/`hash_code` are exposed only as instance methods, not free functions.
pub const OBJECT_BUILTINS: [&str; 2] = [PRINT, PRINTLN];

/// True if `name` is an internal print combinator (`__print` / `__println`).
pub fn is_object_builtin(name: &str) -> bool {
    OBJECT_BUILTINS.contains(&name)
}

/// The low-level character accessor `s.char_at(i)`, a builtin pseudo-method on `string` (like
/// [`SIZE`]); lowered directly to the `$char_at` runtime helper.
pub const CHAR_AT: &str = "char_at";

/// The generic array-allocation builtin, surfaced as the static method `Buffer.alloc<T>(len)`.
pub const ARRAY_NEW: &str = "array_new";

// --- Builtin pseudo-methods on language types -----------------------------------------------
// Recognized on built-in types rather than declared as user methods.

/// `string.size()` / `T[].size()`: the element-count accessor on strings and arrays. The same name
/// is used by the stdlib `List`/`Map` methods, so `size()` is uniform across every collection.
pub const SIZE: &str = "size";

// --- Async intrinsics -----------------------------------------------------------------------

/// `sleep(ms)`: the async timer intrinsic (a free function, unlike the `Promise.*` combinators).
pub const SLEEP: &str = "sleep";

/// Internal free-function names the async combinators lower to (`build_async_intrinsic_call`).
pub const PROMISE_ALL: &str = "__promise_all";
pub const PROMISE_ANY: &str = "__promise_any";
pub const PROMISE_RACE: &str = "__promise_race";

// --- `@intrinsic("…")` static-method registry ----------------------------------------------
// The string inside `@intrinsic("…")` on a stdlib static method. Both semantics (typing) and
// codegen (lowering) classify the attribute key through [`IntrinsicOp`] so the set of attributed
// intrinsics lives in exactly one place instead of being duplicated as bare string `match`es.

pub const ATTR_PRINT: &str = "print";
pub const ATTR_PRINTLN: &str = "println";
pub const ATTR_PROMISE_ALL: &str = "promise_all";
pub const ATTR_PROMISE_ANY: &str = "promise_any";
pub const ATTR_PROMISE_RACE: &str = "promise_race";
pub const ATTR_JSON_SERIALIZE: &str = "json_serialize";
pub const ATTR_JSON_DESERIALIZE: &str = "json_deserialize";
/// `Buffer.alloc<T>(len)` — allocate a zero-initialized array.
pub const ATTR_ARRAY_NEW: &str = "array_new";
/// `Time.sleep(ms)` — the async timer (yields `Future<void>`).
pub const ATTR_SLEEP: &str = "sleep";
/// `String.alloc(n)` / `String.set(s, i, c)` — low-level string buffer primitives.
pub const ATTR_STRING_ALLOC: &str = "string_alloc";
pub const ATTR_STRING_SET: &str = "string_set";
/// `Debug.free_list_head()` — allocator introspection for tests.
pub const ATTR_DEBUG_FREE_LIST: &str = "debug_get_free_list_head";
/// `Debug.heap_ptr()` — current bump-pointer (heap high-water mark).
pub const ATTR_DEBUG_HEAP_PTR: &str = "debug_get_heap_ptr";
/// `Debug.live_objects()` — number of blocks currently allocated (not yet freed).
pub const ATTR_DEBUG_LIVE_OBJECTS: &str = "debug_get_live_objects";
/// `Debug.total_allocations()` — monotonic count of every allocation ever made.
pub const ATTR_DEBUG_TOTAL_ALLOCATIONS: &str = "debug_get_total_allocations";
/// `Debug.ref_count(o)` — live reference count of a heap value.
pub const ATTR_DEBUG_REF_COUNT: &str = "debug_get_ref_count";
/// `Bytes.of<T>(v)` — raw-copy a blittable value's bytes into a fresh `byte[]` buffer.
pub const ATTR_TO_BYTES: &str = "to_bytes";
/// `Bytes.to<T>(bytes)` — reconstruct a blittable value from a `byte[]` buffer (a raw copy
/// of the buffer's payload into a fresh block of `T`'s size).
pub const ATTR_FROM_BYTES: &str = "from_bytes";

/// The operation a `@intrinsic("…")`-tagged static method lowers to. Derived once from the
/// attribute key via [`IntrinsicOp::from_key`], so every layer dispatches off the same enum
/// rather than re-matching raw strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrinsicOp {
    /// `System.print(x)` — print without trailing newline.
    Print,
    /// `System.println(x)` — print with trailing newline.
    Println,
    /// `Promise.all(xs)` — await all, yield `Future<T[]>`.
    PromiseAll,
    /// `Promise.any(xs)` — first to settle, yield `Future<T>`.
    PromiseAny,
    /// `Promise.race(xs)` — first to settle, yield `Future<T>`.
    PromiseRace,
    /// `JSON.serialize<T>(x)` — `T` to its JSON string.
    JsonSerialize,
    /// `JSON.deserialize<T>(s)` — JSON string to `T`.
    JsonDeserialize,
    /// `Buffer.alloc<T>(len)` — allocate a zero-initialized `T[]`.
    ArrayNew,
    /// `Time.sleep(ms)` — async timer yielding `Future<void>`.
    Sleep,
    /// `String.alloc(n)` — allocate an `n`-char string buffer.
    StringAlloc,
    /// `String.set(s, i, c)` — write char `c` at index `i` of buffer `s`.
    StringSet,
    /// `Debug.free_list_head()` — head of the allocator free list.
    DebugFreeList,
    /// `Debug.heap_ptr()` — current bump-pointer value.
    DebugHeapPtr,
    /// `Debug.live_objects()` — number of currently live (un-freed) blocks.
    DebugLiveObjects,
    /// `Debug.total_allocations()` — monotonic allocation count.
    DebugTotalAllocations,
    /// `Debug.ref_count(o)` — live reference count of a heap value.
    DebugRefCount,
    /// `Bytes.of<T>(v)` — raw-copy a blittable value's bytes into a fresh `byte[]`.
    ToBytes,
    /// `Bytes.to<T>(bytes)` — reconstruct a blittable value of `T` from a `byte[]` buffer.
    FromBytes,
}

impl IntrinsicOp {
    /// Classifies an `@intrinsic("key")` attribute value, or `None` if `key` is unknown.
    pub fn from_key(key: &str) -> Option<IntrinsicOp> {
        Some(match key {
            ATTR_PRINT => IntrinsicOp::Print,
            ATTR_PRINTLN => IntrinsicOp::Println,
            ATTR_PROMISE_ALL => IntrinsicOp::PromiseAll,
            ATTR_PROMISE_ANY => IntrinsicOp::PromiseAny,
            ATTR_PROMISE_RACE => IntrinsicOp::PromiseRace,
            ATTR_JSON_SERIALIZE => IntrinsicOp::JsonSerialize,
            ATTR_JSON_DESERIALIZE => IntrinsicOp::JsonDeserialize,
            ATTR_ARRAY_NEW => IntrinsicOp::ArrayNew,
            ATTR_SLEEP => IntrinsicOp::Sleep,
            ATTR_STRING_ALLOC => IntrinsicOp::StringAlloc,
            ATTR_STRING_SET => IntrinsicOp::StringSet,
            ATTR_DEBUG_FREE_LIST => IntrinsicOp::DebugFreeList,
            ATTR_DEBUG_HEAP_PTR => IntrinsicOp::DebugHeapPtr,
            ATTR_DEBUG_LIVE_OBJECTS => IntrinsicOp::DebugLiveObjects,
            ATTR_DEBUG_TOTAL_ALLOCATIONS => IntrinsicOp::DebugTotalAllocations,
            ATTR_DEBUG_REF_COUNT => IntrinsicOp::DebugRefCount,
            ATTR_TO_BYTES => IntrinsicOp::ToBytes,
            ATTR_FROM_BYTES => IntrinsicOp::FromBytes,
            _ => return None,
        })
    }

    /// Classifies the `@intrinsic` attribute on a declaration directly.
    pub fn from_attributes(
        attributes: &[crate::syntax::nodes::AttributeNode],
    ) -> Option<IntrinsicOp> {
        intrinsic_key(attributes)
            .as_deref()
            .and_then(IntrinsicOp::from_key)
    }

    /// For the async combinators, the internal `__promise_*` free-function name they delegate to
    /// (used by both the type checker and codegen); `None` for non-combinator ops.
    pub fn promise_combinator(self) -> Option<&'static str> {
        Some(match self {
            IntrinsicOp::PromiseAll => PROMISE_ALL,
            IntrinsicOp::PromiseAny => PROMISE_ANY,
            IntrinsicOp::PromiseRace => PROMISE_RACE,
            _ => return None,
        })
    }
}
