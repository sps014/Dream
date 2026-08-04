//! Linear-memory marshaling shared by every host-function module: reading/writing Dream strings
//! and `char[]` byte arrays across the WASM boundary. These mirror `DreamInstance`'s helpers in
//! `runtime/dream.js` so the native and JS hosts lay out values identically.
//!
//! Every module's linear memory is a `SharedMemory` (`execution::host::shared_memory`), never a
//! plain `wasmtime::Memory` — `Memory::data`/`data_mut` `debug_assert!`s that the backing memory is
//! *not* shared, so it cannot be used here at all. [`shared_bytes`]/[`shared_bytes_mut`] are the
//! single place that casts `SharedMemory`'s `&[UnsafeCell<u8>]` view to ordinary byte slices.

use crate::mir::abi;
use std::slice;
use wasmtime::*;

/// Casts a `SharedMemory`'s `&[UnsafeCell<u8>]` view to an ordinary read-only `&[u8]`.
///
/// Safe under this project's current concurrency model: through Phase 1, `WebWorker` bodies only
/// exchange copied strings/bytes over channels, so no two threads read/write the *same* address
/// concurrently. Real cross-thread aliasing (`@shared`/`lock`, a later phase) will need real atomic
/// loads at the WAT level for the guest and is a separate concern from this host-side marshaling.
pub fn shared_bytes(memory: &SharedMemory) -> &[u8] {
    let cells = memory.data();
    unsafe { slice::from_raw_parts(cells.as_ptr().cast::<u8>(), cells.len()) }
}

/// Mutable counterpart of [`shared_bytes`]; see its safety note.
///
/// `SharedMemory::data` deliberately takes `&self` (not `&mut self`) since concurrent aliasing from
/// other threads is the entire point of a shared memory — that's exactly why we need a `&mut [u8]`
/// out of it here, so `clippy::mut_from_ref`'s usual soundness concern (two live `&mut` from one
/// `&`) does not apply to this specific, deliberately-shared API.
#[allow(clippy::mut_from_ref)]
pub fn shared_bytes_mut(memory: &SharedMemory) -> &mut [u8] {
    let cells = memory.data();
    unsafe { slice::from_raw_parts_mut(cells.as_ptr().cast::<u8>().cast_mut(), cells.len()) }
}

/// The heap-block tag codegen uses for strings. A host that allocates a string into linear memory
/// must tag the block with this so the runtime treats it as a string.
const TAG_STRING: i32 = abi::TAG_STRING;

/// The heap-block tag codegen uses for arrays. A `char[]` (byte array) is laid out as
/// `[count: i32][bytes...]` at the data pointer.
const TAG_ARRAY: i32 = abi::TAG_ARRAY;

/// Byte size of the length/count prefix at a string/array data pointer (`[len:i32][payload...]`).
const LEN_PREFIX: usize = abi::LEN_PREFIX_SIZE as usize;

/// Reads the little-endian length/count prefix at `base` in `data`, returning `None` if `base` is
/// out of range or the prefix is negative. Shared by the string and byte-array readers so a
/// malformed pointer from a miscompiled/hand-edited module yields an empty value instead of a panic.
fn read_len_prefix(data: &[u8], base: usize) -> Option<usize> {
    let end = base.checked_add(LEN_PREFIX)?;
    if end > data.len() {
        return None;
    }
    let len = i32::from_le_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]);
    (len >= 0).then_some(len as usize)
}

/// Reads a Dream `string` from `memory` at data pointer `ptr`. Layout: `[len: i32][utf8...]`,
/// so the length prefix gives the byte count directly (no NUL terminator). A negative or
/// out-of-bounds pointer yields an empty string rather than panicking.
pub fn read_string_from_memory(memory: &SharedMemory, ptr: i32) -> String {
    let data = shared_bytes(memory);
    if ptr < 0 {
        return String::new();
    }
    let base = ptr as usize;
    let Some(len) = read_len_prefix(data, base) else {
        return String::new();
    };
    let start = base + LEN_PREFIX;
    let end = start.saturating_add(len).min(data.len());
    String::from_utf8_lossy(&data[start..end]).into_owned()
}

/// Resolves the caller module's exported linear `memory`, or a wasm trap (`Err`) if it is absent —
/// so a malformed/foreign module traps the calling task instead of aborting the whole host process.
fn required_memory(caller: &mut Caller<'_, ()>) -> Result<SharedMemory> {
    caller
        .get_export(abi::EXPORT_MEMORY)
        .and_then(Extern::into_shared_memory)
        .ok_or_else(|| Error::msg("module must export `memory`"))
}

/// Resolves the caller module's exported `malloc` with the expected `(size, tag) -> ptr` signature,
/// or a wasm trap (`Err`) if it is missing or mistyped.
fn required_malloc(caller: &mut Caller<'_, ()>) -> Result<TypedFunc<(i32, i32), i32>> {
    caller
        .get_export(abi::EXPORT_MALLOC)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg("module must export `malloc`"))?
        .typed::<(i32, i32), i32>(&*caller)
        .map_err(|_| Error::msg("unexpected `malloc` signature"))
}

/// Reads the caller's exported `memory` and returns the length-prefixed string at `ptr`. Traps
/// (`Err`) only if the module does not export `memory`.
pub(crate) fn read_arg_string(caller: &mut Caller<'_, ()>, ptr: i32) -> Result<String> {
    let memory = required_memory(caller)?;
    Ok(read_string_from_memory(&memory, ptr))
}

/// Allocates `s` as a Dream `string` inside the module's linear memory by calling its exported
/// `malloc`, storing the length prefix, and copying the UTF-8 bytes at `ptr+4`. Returns the data
/// pointer (mirrors `DreamInstance.writeString` in `runtime/dream.js`). Used by host functions that
/// return strings. Layout: `[len: i32][utf8...]` (no NUL terminator).
pub fn write_string_to_memory(caller: &mut Caller<'_, ()>, s: &str) -> Result<i32> {
    let malloc = required_malloc(caller)?;
    let bytes = s.as_bytes();
    let ptr = malloc.call(
        &mut *caller,
        (LEN_PREFIX as i32 + bytes.len() as i32, TAG_STRING),
    )?;
    let memory = required_memory(caller)?;
    let start = ptr as usize;
    let data = shared_bytes_mut(&memory);
    data[start..start + LEN_PREFIX].copy_from_slice(&(bytes.len() as i32).to_le_bytes());
    data[start + LEN_PREFIX..start + LEN_PREFIX + bytes.len()].copy_from_slice(bytes);
    Ok(ptr)
}

/// Reads a Dream `char[]` (byte array) at data pointer `ptr` into a `Vec<u8>` with a single bulk
/// copy. Layout: `[count: i32][bytes...]` (char elements are 1 byte). No string round-trip, so
/// this is binary-safe.
pub(crate) fn read_arg_bytes(caller: &mut Caller<'_, ()>, ptr: i32) -> Result<Vec<u8>> {
    let memory = required_memory(caller)?;
    let data = shared_bytes(&memory);
    if ptr < 0 {
        return Ok(Vec::new());
    }
    let base = ptr as usize;
    let Some(count) = read_len_prefix(data, base) else {
        return Ok(Vec::new());
    };
    let start = base + LEN_PREFIX;
    let end = start.saturating_add(count).min(data.len());
    Ok(data[start..end].to_vec())
}

/// Allocates a Dream `char[]` (byte array) holding `bytes` via the module's exported `malloc`,
/// with a single bulk copy. Returns the array data pointer. Mirrors `DreamInstance.writeArray`
/// in `runtime/dream.js`.
pub fn write_bytes_to_memory(caller: &mut Caller<'_, ()>, bytes: &[u8]) -> Result<i32> {
    let malloc = required_malloc(caller)?;
    let count = bytes.len() as i32;
    let ptr = malloc.call(&mut *caller, (LEN_PREFIX as i32 + count, TAG_ARRAY))?;
    let memory = required_memory(caller)?;
    let base = ptr as usize;
    let data = shared_bytes_mut(&memory);
    data[base..base + LEN_PREFIX].copy_from_slice(&count.to_le_bytes());
    data[base + LEN_PREFIX..base + LEN_PREFIX + bytes.len()].copy_from_slice(bytes);
    Ok(ptr)
}
