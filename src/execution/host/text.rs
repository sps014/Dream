//! Unicode text helpers (the `Dream` module behind `system.text.Unicode`). Heavy normalization,
//! case folding, and grapheme segmentation use mature Rust crates; `runtime/dream.js` mirrors the
//! same ABI with `String.normalize`, `toLocaleLowerCase`, and `Intl.Segmenter`.

use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use wasmtime::*;

use super::memory::{read_arg_string, write_string_to_memory};

fn write_string_array_to_memory(caller: &mut Caller<'_, ()>, items: &[String]) -> Result<i32> {
    use dream_mir::abi;
    let malloc = caller
        .get_export(abi::EXPORT_MALLOC)
        .and_then(Extern::into_func)
        .ok_or_else(|| Error::msg("module must export `malloc`"))?
        .typed::<(i32, i32), i32>(&*caller)
        .map_err(|_| Error::msg("unexpected `malloc` signature"))?;
    let count = items.len() as i32;
    let ptr = malloc.call(&mut *caller, (4 + count * 4, abi::TAG_ARRAY))?;
    let memory = caller
        .get_export(abi::EXPORT_MEMORY)
        .and_then(Extern::into_shared_memory)
        .ok_or_else(|| Error::msg("module must export `memory`"))?;
    let data = super::memory::shared_bytes_mut(&memory);
    let base = ptr as usize;
    data[base..base + 4].copy_from_slice(&count.to_le_bytes());
    for (i, item) in items.iter().enumerate() {
        let elem_ptr = write_string_to_memory(caller, item)?;
        let off = base + 4 + i * 4;
        data[off..off + 4].copy_from_slice(&elem_ptr.to_le_bytes());
    }
    Ok(ptr)
}

fn normalize_string(text: &str, form: i32) -> String {
    match form {
        1 => text.nfd().collect(),
        2 => text.nfkc().collect(),
        3 => text.nfkd().collect(),
        _ => text.nfc().collect(),
    }
}

/// Registers `Unicode.*` host functions on `linker`.
pub fn link_text_functions(linker: &mut Linker<()>) -> Result<()> {
    linker.func_wrap("Dream", "unicodeNormalize", |mut caller: Caller<'_, ()>, ptr: i32, form: i32| {
        let text = read_arg_string(&mut caller, ptr)?;
        let normalized = normalize_string(&text, form);
        write_string_to_memory(&mut caller, &normalized)
    })?;

    linker.func_wrap("Dream", "unicodeToLower", |mut caller: Caller<'_, ()>, ptr: i32| {
        let text = read_arg_string(&mut caller, ptr)?;
        let lowered = text.to_lowercase();
        write_string_to_memory(&mut caller, &lowered)
    })?;

    linker.func_wrap(
        "Dream",
        "unicodeGraphemes",
        |mut caller: Caller<'_, ()>, ptr: i32| {
            let text = read_arg_string(&mut caller, ptr)?;
            let graphemes: Vec<String> = text.graphemes(true).map(str::to_string).collect();
            write_string_array_to_memory(&mut caller, &graphemes)
        },
    )?;

    Ok(())
}
