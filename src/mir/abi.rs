//! Runtime ABI constants shared between the MIR backend and the embedded runtime `.wat` layers.
//!
//! Every heap block carries a type tag in its header (`[size][tag][ref_count]`). Reference types
//! store their tag in the block they already own; primitives are boxed into a small tagged block.
//! These are the single source of truth for those tags — the `{TAG_*}` placeholders in
//! `runtime/object.wat` / `runtime/format.wat` are substituted from them at emit time, and the host
//! interop layer (`execution/host`) mirrors the same values.

pub const TAG_INT: i32 = 1;
pub const TAG_FLOAT: i32 = 2;
pub const TAG_DOUBLE: i32 = 3;
pub const TAG_BOOL: i32 = 4;
pub const TAG_STRING: i32 = 5;
pub const TAG_ARRAY: i32 = 6;
pub const TAG_CHAR: i32 = 7;
pub const TAG_LONG: i32 = 8;
pub const TAG_UINT: i32 = 9;
pub const TAG_ULONG: i32 = 10;
pub const TAG_BYTE: i32 = 11;
/// Structs/unions are assigned consecutive tags starting here, ordered by sorted type name.
pub const TAG_STRUCT_BASE: i32 = 12;

// -- Heap block layout ---------------------------------------------------------------------------
//
// Every allocated value is preceded by a fixed header `[size:i32][tag:i32][ref_count:i32]`. These
// offsets are the single source of truth shared by the emitter, the host interop layer
// (`execution/host`), and the hand-written runtime `.wat` (which references them via `{...}`
// placeholders substituted at emit time, or via matching comments).

/// Byte size of the universal heap-block header `[size:i32][tag:i32][ref_count:i32]`. A value's data
/// pointer is `block_start + HEAP_HEADER_SIZE`.
pub const HEAP_HEADER_SIZE: u32 = 12;

/// Byte offset (from the block start) of the type-tag word in the heap header.
pub const HEADER_TAG_OFFSET: u32 = 4;

/// Byte offset (from the block start) of the reference-count word in the heap header.
pub const HEADER_REFCOUNT_OFFSET: u32 = 8;

/// Byte size of the length prefix preceding a `string`'s utf8 bytes / an array's elements at the
/// data pointer (`[len:i32][payload...]`); the payload starts at `ptr + LEN_PREFIX_SIZE`.
pub const LEN_PREFIX_SIZE: u32 = 4;

// -- Linear memory -------------------------------------------------------------------------------
//
// Layout, low -> high address:
//
//   [ static data ] [ shadow stack -> grows DOWN ] [ heap -> grows UP (memory.grow) ]
//   ^ strings+itables ^ SHADOW_STACK_SIZE bytes     ^ heap base == shadow-stack top
//
// The shadow stack (inline value-`struct` locals) and the heap share a single boundary and grow
// away from it in *opposite* directions, so they can never collide. The shadow stack is capped at
// `SHADOW_STACK_SIZE` (a deep-recursion bound); the heap is effectively unbounded, extending linear
// memory via `memory.grow` in the allocator's bump path.

/// WASM linear-memory page size, in bytes.
pub const WASM_PAGE_SIZE: u32 = 65536;

/// Bytes reserved for the shadow stack. It occupies its own region just above the static data and
/// grows *downward*; the heap base sits at the top of this region. Sized to comfortably hold deep
/// value-`struct` recursion (overflowing it is a stack-overflow bug, not a heap/stack collision).
pub const SHADOW_STACK_SIZE: u32 = 16 * WASM_PAGE_SIZE; // 1 MiB

/// Pages of heap mapped in the initial memory, beyond the static-data + shadow-stack regions. The
/// heap grows past this on demand via `memory.grow`, so this is only a starting cushion.
pub const INITIAL_HEAP_PAGES: u32 = 1;

/// Base address (block start) of the interned string data segment; the heap begins above it.
pub const STRING_BASE: u32 = 1024;

// -- Runtime export / import symbol names --------------------------------------------------------
//
// The names below form the contract between the emitted module and every host (`execution/host`,
// `wasm_runner`, `runtime/dream.js`) plus the passes that special-case the entry point. Keeping
// them here means a rename is a single edit.

/// The program entry point exported to, and invoked by, the host.
pub const ENTRY_FN: &str = "main";

/// Host import module for the fixed `print_*` builtins.
pub const ENV_MODULE: &str = "env";

/// Exported allocator entry points the host uses to build heap values.
pub const EXPORT_MALLOC: &str = "malloc";
pub const EXPORT_FREE: &str = "free";
/// Exported linear memory.
pub const EXPORT_MEMORY: &str = "memory";

/// Async-runtime exports the host scheduler bridge drives (see `execution/host/http.rs` and
/// `runtime/dream.js`).
pub const EXPORT_RUN_LOOP: &str = "__dream_run_loop";
pub const EXPORT_RESOLVE: &str = "__dream_resolve";
pub const EXPORT_NEW_FUTURE: &str = "__dream_new_future";
