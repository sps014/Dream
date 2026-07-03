//! Linear-memory marshaling shared by every host-function module: reading/writing Dream strings
//! and `char[]` byte arrays across the WASM boundary. These mirror `DreamInstance`'s helpers in
//! `runtime/dream.js` so the native and JS hosts lay out values identically.

use crate::mir::abi;
use wasmtime::*;

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
pub fn read_string_from_memory(memory: &Memory, store: impl AsContext, ptr: i32) -> String {
    let data = memory.data(&store);
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

/// Reads the caller's exported `memory` and returns the length-prefixed string at `ptr`.
pub(crate) fn read_arg_string(caller: &mut Caller<'_, ()>, ptr: i32) -> String {
    let memory = caller
        .get_export(abi::EXPORT_MEMORY)
        .and_then(Extern::into_memory)
        .expect("module must export `memory`");
    read_string_from_memory(&memory, &*caller, ptr)
}

/// Allocates `s` as a Dream `string` inside the module's linear memory by calling its exported
/// `malloc`, storing the length prefix, and copying the UTF-8 bytes at `ptr+4`. Returns the data
/// pointer (mirrors `DreamInstance.writeString` in `runtime/dream.js`). Used by host functions that
/// return strings. Layout: `[len: i32][utf8...]` (no NUL terminator).
pub fn write_string_to_memory(caller: &mut Caller<'_, ()>, s: &str) -> i32 {
    let malloc = caller
        .get_export(abi::EXPORT_MALLOC)
        .and_then(Extern::into_func)
        .expect("module must export `malloc`")
        .typed::<(i32, i32), i32>(&*caller)
        .expect("unexpected `malloc` signature");
    let bytes = s.as_bytes();
    let ptr = malloc
        .call(&mut *caller, (LEN_PREFIX as i32 + bytes.len() as i32, TAG_STRING))
        .expect("malloc call failed");
    let memory = caller
        .get_export(abi::EXPORT_MEMORY)
        .and_then(Extern::into_memory)
        .expect("module must export `memory`");
    let start = ptr as usize;
    let data = memory.data_mut(&mut *caller);
    data[start..start + LEN_PREFIX].copy_from_slice(&(bytes.len() as i32).to_le_bytes());
    data[start + LEN_PREFIX..start + LEN_PREFIX + bytes.len()].copy_from_slice(bytes);
    ptr
}

/// Reads a Dream `char[]` (byte array) at data pointer `ptr` into a `Vec<u8>` with a single bulk
/// copy. Layout: `[count: i32][bytes...]` (char elements are 1 byte). No string round-trip, so
/// this is binary-safe.
pub(crate) fn read_arg_bytes(caller: &mut Caller<'_, ()>, ptr: i32) -> Vec<u8> {
    let memory = caller
        .get_export(abi::EXPORT_MEMORY)
        .and_then(Extern::into_memory)
        .expect("module must export `memory`");
    let data = memory.data(&*caller);
    if ptr < 0 {
        return Vec::new();
    }
    let base = ptr as usize;
    let Some(count) = read_len_prefix(data, base) else {
        return Vec::new();
    };
    let start = base + LEN_PREFIX;
    let end = start.saturating_add(count).min(data.len());
    data[start..end].to_vec()
}

/// Allocates a Dream `char[]` (byte array) holding `bytes` via the module's exported `malloc`,
/// with a single bulk copy. Returns the array data pointer. Mirrors `DreamInstance.writeArray`
/// in `runtime/dream.js`.
pub fn write_bytes_to_memory(caller: &mut Caller<'_, ()>, bytes: &[u8]) -> i32 {
    let malloc = caller
        .get_export(abi::EXPORT_MALLOC)
        .and_then(Extern::into_func)
        .expect("module must export `malloc`")
        .typed::<(i32, i32), i32>(&*caller)
        .expect("unexpected `malloc` signature");
    let count = bytes.len() as i32;
    let ptr = malloc
        .call(&mut *caller, (LEN_PREFIX as i32 + count, TAG_ARRAY))
        .expect("malloc call failed");
    let memory = caller
        .get_export(abi::EXPORT_MEMORY)
        .and_then(Extern::into_memory)
        .expect("module must export `memory`");
    let base = ptr as usize;
    let data = memory.data_mut(&mut *caller);
    data[base..base + LEN_PREFIX].copy_from_slice(&count.to_le_bytes());
    data[base + LEN_PREFIX..base + LEN_PREFIX + bytes.len()].copy_from_slice(bytes);
    ptr
}
