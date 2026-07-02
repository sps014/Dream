//! Linear-memory marshaling shared by every host-function module: reading/writing Dream strings
//! and `char[]` byte arrays across the WASM boundary. These mirror `DreamInstance`'s helpers in
//! `runtime/dream.js` so the native and JS hosts lay out values identically.

use wasmtime::*;

/// The heap-block tag codegen uses for strings (mirrors `mir::abi::TAG_STRING`).
/// A host that allocates a string into linear memory must tag the block with this so the runtime
/// treats it as a string.
const TAG_STRING: i32 = 5;

/// The heap-block tag codegen uses for arrays (mirrors `mir::abi::TAG_ARRAY`). A
/// `char[]` (byte array) is laid out as `[count: i32][bytes...]` at the data pointer.
const TAG_ARRAY: i32 = 6;

/// Reads a Dream `string` from `memory` at data pointer `ptr`. Layout: `[len: i32][utf8...]`,
/// so the length prefix gives the byte count directly (no NUL terminator).
pub fn read_string_from_memory(memory: &Memory, store: impl AsContext, ptr: i32) -> String {
    let data = memory.data(&store);
    let base = ptr as usize;
    let len =
        i32::from_le_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]) as usize;
    let start = base + 4;
    let end = (start + len).min(data.len());
    String::from_utf8_lossy(&data[start..end]).into_owned()
}

/// Reads the caller's exported `memory` and returns the length-prefixed string at `ptr`.
pub(crate) fn read_arg_string(caller: &mut Caller<'_, ()>, ptr: i32) -> String {
    let memory = caller
        .get_export("memory")
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
        .get_export("malloc")
        .and_then(Extern::into_func)
        .expect("module must export `malloc`")
        .typed::<(i32, i32), i32>(&*caller)
        .expect("unexpected `malloc` signature");
    let bytes = s.as_bytes();
    let ptr = malloc
        .call(&mut *caller, (4 + bytes.len() as i32, TAG_STRING))
        .expect("malloc call failed");
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .expect("module must export `memory`");
    let start = ptr as usize;
    let data = memory.data_mut(&mut *caller);
    data[start..start + 4].copy_from_slice(&(bytes.len() as i32).to_le_bytes());
    data[start + 4..start + 4 + bytes.len()].copy_from_slice(bytes);
    ptr
}

/// Reads a Dream `char[]` (byte array) at data pointer `ptr` into a `Vec<u8>` with a single bulk
/// copy. Layout: `[count: i32][bytes...]` (char elements are 1 byte). No string round-trip, so
/// this is binary-safe.
pub(crate) fn read_arg_bytes(caller: &mut Caller<'_, ()>, ptr: i32) -> Vec<u8> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .expect("module must export `memory`");
    let data = memory.data(&*caller);
    let base = ptr as usize;
    let count =
        i32::from_le_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]) as usize;
    let start = base + 4;
    data[start..start + count].to_vec()
}

/// Allocates a Dream `char[]` (byte array) holding `bytes` via the module's exported `malloc`,
/// with a single bulk copy. Returns the array data pointer. Mirrors `DreamInstance.writeArray`
/// in `runtime/dream.js`.
pub fn write_bytes_to_memory(caller: &mut Caller<'_, ()>, bytes: &[u8]) -> i32 {
    let malloc = caller
        .get_export("malloc")
        .and_then(Extern::into_func)
        .expect("module must export `malloc`")
        .typed::<(i32, i32), i32>(&*caller)
        .expect("unexpected `malloc` signature");
    let count = bytes.len() as i32;
    let ptr = malloc
        .call(&mut *caller, (4 + count, TAG_ARRAY))
        .expect("malloc call failed");
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .expect("module must export `memory`");
    let base = ptr as usize;
    let data = memory.data_mut(&mut *caller);
    data[base..base + 4].copy_from_slice(&count.to_le_bytes());
    data[base + 4..base + 4 + bytes.len()].copy_from_slice(bytes);
    ptr
}
