//! MIR -> WAT (text WebAssembly) backend.
//!
//! The relooper ([`super::relooper`]) recovers structured shapes from the CFG; this emitter lowers a
//! function to WAT. Control flow uses a labeled-block dispatch loop (a `br_table` over a block-index
//! local), which is correct for any reducible CFG; the relooper shapes are the basis for emitting
//! idiomatic structured `block`/`loop`/`if` instead, the planned refinement. Straight-line
//! statements, operands, and arithmetic are emitted directly. Memory-backed places (struct fields,
//! array elements) and allocation reuse the existing runtime/object/string layers when wired in;
//! they are marked `;; TODO(layout)` here pending that integration.

use super::{
    BinOp, Const, MirFunction, Operand, Place, Rvalue, Statement, Terminator, UnOp,
};
use crate::hir::{scalar_size, LayoutTable};
use crate::types::{DefId, PrimTy, TyKind, TypeId, TypeInterner};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::fmt::Write;

/// Runtime type tag for arrays passed to `$malloc`, matching the object protocol's `$object_tag`
/// dispatch (see [`super::abi::TAG_ARRAY`]).
const ARRAY_TAG: i32 = super::abi::TAG_ARRAY;

/// The first tag assigned to a user struct/union; consecutive types get consecutive tags, so the
/// shared runtime's dispatch tables agree (see [`super::abi::TAG_STRUCT_BASE`]).
const STRUCT_TAG_BASE: i32 = super::abi::TAG_STRUCT_BASE;

/// The heap-block tag for strings (see [`super::abi::TAG_STRING`]), written into the header of
/// interned string blocks so the runtime treats them as strings.
const STRING_TAG: i32 = super::abi::TAG_STRING;

/// Byte size of the universal heap-block header `[size:i32][tag:i32][ref_count:i32]` that precedes
/// every allocated value; a value's pointer points at `block_start + HEAP_HEADER_SIZE`.
const HEAP_HEADER_SIZE: u32 = 12;

/// Base address (block start) of the interned string data segment. Each string is a heap-object
/// block `[size=0][tag=STRING][ref_count=1][len: i32][utf8]`; the mapped address points at the
/// length word (block start + header), with utf8 bytes at `ptr+4`. `$strlen` is a single load at
/// `ptr`; there is no NUL terminator (the length prefix makes it redundant). The heap starts above.
const STRING_BASE: u32 = 1024;

/// Linear-memory size, in 64 KiB WASM pages.
const MEMORY_PAGES: u32 = 16;

/// The shadow stack (for inline value-`struct` locals) grows *downward* from the top of linear
/// memory, while the heap bump-allocates *upward* from `heap_start`. `$__sp` is initialized here;
/// each function that has value locals reserves its frame by subtracting from `$__sp` in its
/// prologue and restores `$__sp` before every return.
const SHADOW_STACK_TOP: u32 = MEMORY_PAGES * 65536;

/// The fixed allocator runtime (`$malloc`/`$free`/`$retain`/`$release_generic`/`$object_tag`), the
/// single source of truth for the heap ABI. Its debug-counter placeholders are filled in by
/// [`runtime_prelude`] (instrumentation on only under `--debug`).
const RUNTIME_ALLOCATOR: &str = include_str!("../runtime/allocator.wat");

/// The fixed string runtime (`$strlen`/`$char_at`/`$string_eq`/`$concat_strings`/`$string_alloc`/…).
/// Self-contained given the allocator + memory.
const RUNTIME_STRINGS: &str = include_str!("../runtime/strings.wat");

/// The object runtime: box/unbox/hash plus the integer-family `*_to_string` formatters
/// (`$int_to_string`/`$long_to_string`/`$byte_to_string`/…). `{TAG_*}` placeholders are substituted.
const RUNTIME_OBJECT: &str = include_str!("../runtime/object.wat");

/// The decimal `float`/`double` formatter (`$float_to_string`/`$double_to_string`). `{minus}` (the
/// data pointer of the interned `"-"`) and `{TAG_STRING}` are substituted.
const RUNTIME_FORMAT: &str = include_str!("../runtime/format.wat");

/// String constants the `*_to_string` runtime references by address (`bool` renders to `"true"`/
/// `"false"`; the `double` formatter prepends `"-"`). Interned into every module so the runtime is
/// always self-contained.
const RUNTIME_STR_CONSTS: [&str; 3] = ["true", "false", "-"];

mod emitter;
mod module;
mod protocol;
mod release;
mod runtime;
mod strings;
mod tables;
mod types;
mod valuetype;
mod wat_dce;

// Flat internal re-exports so each submodule can `use super::*` and call sibling helpers
// exactly as it did when this was one file. Kept private (not part of the crate API).
use emitter::*;
use protocol::*;
use release::*;
use runtime::*;
use strings::*;
use tables::*;
use types::*;
use valuetype::*;
use wat_dce::*;

// The external API of the backend, at the historical `crate::mir::emit::…` paths.
pub use emitter::emit_function;
pub use module::{emit_module, emit_program};
pub(crate) use emitter::{emit_expr_to_scratch, emit_straight_line_segment};
pub(crate) use tables::{func_symbol, poll_symbol, release_call_for_ty};
pub(crate) use types::wasm_ty_of;

#[cfg(test)]
mod tests;
