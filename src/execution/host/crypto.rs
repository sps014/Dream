//! Cryptographic host functions (the `Dream` module behind `system.crypto`). Digests use the `sha2`
//! and `hmac` crates; CSPRNG uses `getrandom`. Browser/Node hosts mirror these in `runtime/dream.js`.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};
use wasmtime::*;

use super::memory::{read_arg_bytes, shared_bytes_mut, write_bytes_to_memory};

const LEN_PREFIX: usize = dream_mir::abi::LEN_PREFIX_SIZE as usize;

/// Reads the element count prefix at a `byte[]` data pointer in the caller's linear memory.
fn read_byte_array_len(caller: &mut Caller<'_, ()>, ptr: i32) -> Result<usize> {
    let memory = caller
        .get_export(dream_mir::abi::EXPORT_MEMORY)
        .and_then(Extern::into_shared_memory)
        .ok_or_else(|| Error::msg("module must export `memory`"))?;
    let data = super::memory::shared_bytes(&memory);
    if ptr < 0 {
        return Ok(0);
    }
    let base = ptr as usize;
    let end = base.checked_add(LEN_PREFIX).filter(|&e| e <= data.len());
    let Some(_end) = end else {
        return Ok(0);
    };
    let len = i32::from_le_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]);
    Ok((len.max(0)) as usize)
}

/// Overwrites the payload of an existing `byte[]` at `ptr` with `bytes` (truncating to the array length).
fn fill_byte_array_in_memory(
    caller: &mut Caller<'_, ()>,
    ptr: i32,
    bytes: &[u8],
) -> Result<()> {
    let memory = caller
        .get_export(dream_mir::abi::EXPORT_MEMORY)
        .and_then(Extern::into_shared_memory)
        .ok_or_else(|| Error::msg("module must export `memory`"))?;
    let count = read_byte_array_len(caller, ptr)?;
    if count == 0 || ptr < 0 {
        return Ok(());
    }
    let base = ptr as usize;
    let start = base + LEN_PREFIX;
    let data = shared_bytes_mut(&memory);
    let len = count.min(bytes.len()).min(data.len().saturating_sub(start));
    data[start..start + len].copy_from_slice(&bytes[..len]);
    Ok(())
}

/// Registers the `system.crypto` host functions on `linker`.
pub fn link_crypto_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap(
        "Dream",
        "cryptoSha256",
        |mut caller: Caller<'_, ()>, data_ptr: i32| -> Result<i32> {
            let data = read_arg_bytes(&mut caller, data_ptr)?;
            let digest = Sha256::digest(&data);
            write_bytes_to_memory(&mut caller, &digest)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "cryptoSha512",
        |mut caller: Caller<'_, ()>, data_ptr: i32| -> Result<i32> {
            let data = read_arg_bytes(&mut caller, data_ptr)?;
            let digest = Sha512::digest(&data);
            write_bytes_to_memory(&mut caller, &digest)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "cryptoHmacSha256",
        |mut caller: Caller<'_, ()>, key_ptr: i32, data_ptr: i32| -> Result<i32> {
            let key = read_arg_bytes(&mut caller, key_ptr)?;
            let data = read_arg_bytes(&mut caller, data_ptr)?;
            let mut mac =
                Hmac::<Sha256>::new_from_slice(&key).map_err(|_| Error::msg("invalid HMAC key"))?;
            mac.update(&data);
            write_bytes_to_memory(&mut caller, &mac.finalize().into_bytes())
        },
    )?;

    linker.func_wrap(
        "Dream",
        "cryptoSecureRandomBytes",
        |mut caller: Caller<'_, ()>, n: i32| -> Result<i32> {
            let count = if n > 0 { n as usize } else { 0 };
            let mut out = vec![0u8; count];
            if count > 0 {
                getrandom::getrandom(&mut out)
                    .map_err(|e| Error::msg(format!("getrandom failed: {}", e)))?;
            }
            write_bytes_to_memory(&mut caller, &out)
        },
    )?;

    linker.func_wrap(
        "Dream",
        "cryptoSecureRandomFill",
        |mut caller: Caller<'_, ()>, dest_ptr: i32| -> Result<()> {
            let count = read_byte_array_len(&mut caller, dest_ptr)?;
            if count == 0 {
                return Ok(());
            }
            let mut buf = vec![0u8; count];
            getrandom::getrandom(&mut buf)
                .map_err(|e| Error::msg(format!("getrandom failed: {}", e)))?;
            fill_byte_array_in_memory(&mut caller, dest_ptr, &buf)
        },
    )?;

    Ok(())
}
