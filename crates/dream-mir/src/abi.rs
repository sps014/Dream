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

/// Maximum page count declared on the module's linear memory. The WASM threads proposal requires a
/// shared memory to declare a fixed maximum up front (unlike a plain memory, which may leave it
/// unbounded) — this is the wasm32 address-space ceiling (`65536 * 64KiB` = 4 GiB), so it does not
/// otherwise constrain how far the bump-pointer heap (`memory.grow`) can grow.
pub const MAX_MEMORY_PAGES: u32 = 65536;

/// Base address (block start) of the interned string data segment; the heap begins above it.
pub const STRING_BASE: u32 = 1024;

// -- Cross-thread allocator coordination (linear memory is `shared`, see `execution::host::shared_memory`) --
//
// The segregated free-list table occupies bytes [4, 44) (`runtime/allocator.wat`'s slot table: 8
// size-class heads + 1 large-object head). Two more coordination words are reserved right after it,
// both well below `STRING_BASE` (1024) so they can never collide with static data:
//
// - `ALLOC_LOCK_ADDR`: a spinlock (0 = free, 1 = held) serializing every `$malloc`/`$free` body. The
//   owner instance and every `WebWorker` instance of the same module import the *same*
//   `wasmtime::SharedMemory`, so without this lock two threads racing the free-list/bump-pointer
//   logic concurrently would corrupt the heap (lost updates, double-allocated blocks).
// - `HEAP_PTR_ADDR`: the bump-pointer high-water mark, moved out of a WASM global (which is
//   per-*instance*, not per-memory) into shared memory so every thread bumps the same pointer.
//   Initialized exactly once across however many instances share this memory, via an atomic
//   compare-exchange from 0 in `$__runtime_init` (see `emit/module.rs`) — whichever instance runs
//   first wins the exchange, every later instantiation's exchange is a no-op.
pub const ALLOC_LOCK_ADDR: u32 = 44;
pub const HEAP_PTR_ADDR: u32 = 48;

/// A monotonically increasing counter (`i32.atomic.rmw.add`) handing out a small, dense, unique id
/// to each thread that ever calls `$__thread_id` (see `runtime/sync.wat`) — the owner instance and
/// every `WebWorker` instance draw from this one shared word, so ids never collide across threads.
/// Each thread caches its own id in the ordinary (per-*instance*) WASM global `$__tid` after the
/// first call, so every later call is a single `global.get`, not a repeat atomic RMW. Backs the
/// owner-thread-id half of the reentrant lock word (`@shared class`'s embedded lock, `lock (obj)
/// { ... }`, and `Lock`) — see `HEADER_LOCK_WORD_SIZE` below for the lock word's own layout.
pub const THREAD_ID_COUNTER_ADDR: u32 = 52;

// -- `@shared class` header extension --------------------------------------------------------
//
// An `@shared class` instance carries one extra `i32` word, right past its last field (i.e. at
// `data_ptr + TypeLayout::size`, computed per-type at emit time — see `Rvalue::New` emission in
// `src/mir/emit/emitter/rvalue/mod.rs`), reserved as a reentrant lock word backing `lock (obj)
// { ... }` and `Lock`'s `acquire`/`release`. Packed as `(owner_thread_id << LOCK_DEPTH_BITS) |
// recursion_depth`, `0` meaning unlocked. Ordinary (non-`@shared`) classes pay nothing for this —
// their allocation size is exactly their field layout's `size`, no extra word.
pub const HEADER_LOCK_WORD_SIZE: u32 = 4;

/// Bit width of the reentrant lock word's recursion-depth field (low bits); the remaining high
/// bits hold the owning thread's id. `1 << LOCK_DEPTH_BITS` is both the max recursion depth and
/// the max distinct thread ids this scheme supports — 65536 of each is far beyond any realistic
/// nesting depth or worker-thread count.
pub const LOCK_DEPTH_BITS: u32 = 16;

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

/// Worker-thread trampoline export (see `src/stdlib/core/webworker.dream`). The *native* host
/// worker driver (`execution/host/worker.rs`) calls this with a body funcref index and a message
/// string pointer; it performs one `call_indirect` on the `fun(string): string` body — driving an
/// async body's constructor to completion in place if the call_indirect result turns out to be an
/// untagged `Future` frame rather than the real value (see `src/mir/emit/module.rs`) — and returns
/// the reply string pointer. Kept a fixed export so a freshly instantiated worker instance of the
/// same module can be driven entirely from the host. Sound only because every native host `async`
/// op resolves synchronously before returning to WASM; the browser worker driver (`runtime/dream.js`)
/// cannot assume that, so it calls [`EXPORT_WORKER_INVOKE_RAW`] instead and drives completion itself.
pub const EXPORT_WORKER_INVOKE: &str = "__dream_worker_invoke";

/// The same one `call_indirect` `__dream_worker_invoke` performs, minus its synchronous
/// drive-to-completion step — the raw constructor/function return value, un-interpreted. Used only
/// by the browser worker driver (`runtime/dream.js`), which must instead check whether the result
/// is a still-pending `Future` and await it asynchronously (a real `extern async` host call there
/// settles later via a Promise callback, never synchronously within the `call_indirect`).
pub const EXPORT_WORKER_INVOKE_RAW: &str = "__dream_worker_invoke_raw";
