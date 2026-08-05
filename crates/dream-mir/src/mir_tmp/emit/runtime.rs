use super::*;

/// The allocator + string runtime. When `debug` is on (the default compiler mode), `$malloc` bumps
/// `$live_objects`/`$total_allocations` and `$free` decrements `$live_objects` (backing the
/// `Debug.*` probes); under `--release` the placeholders expand to nothing so the hot allocation
/// path carries no extra instructions.
pub(super) fn runtime_prelude(debug: bool) -> String {
    let (malloc_count, free_count) = if debug {
        (
            "global.get $live_objects\n    i32.const 1\n    i32.add\n    global.set $live_objects\n    \
             global.get $total_allocations\n    i32.const 1\n    i32.add\n    global.set $total_allocations",
            "global.get $live_objects\n    i32.const 1\n    i32.sub\n    global.set $live_objects",
        )
    } else {
        ("", "")
    };
    let mut out = RUNTIME_ALLOCATOR
        .replace(";;@DEBUG_ALLOC_COUNT@", malloc_count)
        .replace(";;@DEBUG_FREE_COUNT@", free_count)
        .replace(
            "{ALLOC_LOCK_ADDR}",
            &crate::mir::abi::ALLOC_LOCK_ADDR.to_string(),
        )
        .replace("{HEAP_PTR_ADDR}", &crate::mir::abi::HEAP_PTR_ADDR.to_string());
    out.push('\n');
    // The string runtime tags freshly allocated string blocks with the heap `TAG_STRING`. `$char_at`
    // itself no longer bounds-checks: callers emit a located check inline before calling it (see
    // `Emitter::emit_char_at`), so a string-index panic gets a precise file:line rather than the one
    // bare, unlocated message a truly shared runtime helper would be stuck with.
    out.push_str(
        &RUNTIME_STRINGS
            .replace("{TAG_STRING}", &crate::mir::abi::TAG_STRING.to_string())
            .replace("{HEAP_PTR_ADDR}", &crate::mir::abi::HEAP_PTR_ADDR.to_string()),
    );
    out
}

/// Builds the `*_to_string` runtime (object formatters + generated `$bool_to_string` + the float/
/// double formatter), resolving the `{TAG_*}`/`{minus}` placeholders and the `bool` string pointers
/// from the interned string table. Depends on the allocator + string runtime emitted before it.
pub(super) fn to_string_runtime(strings: &IndexMap<String, u32>) -> String {
    use crate::mir::abi as tags;
    let object = RUNTIME_OBJECT
        .replace("{TAG_INT}", &tags::TAG_INT.to_string())
        .replace("{TAG_FLOAT}", &tags::TAG_FLOAT.to_string())
        .replace("{TAG_DOUBLE}", &tags::TAG_DOUBLE.to_string())
        .replace("{TAG_BOOL}", &tags::TAG_BOOL.to_string())
        .replace("{TAG_STRING}", &tags::TAG_STRING.to_string())
        .replace("{TAG_CHAR}", &tags::TAG_CHAR.to_string())
        .replace("{TAG_LONG}", &tags::TAG_LONG.to_string())
        .replace("{TAG_UINT}", &tags::TAG_UINT.to_string())
        .replace("{TAG_ULONG}", &tags::TAG_ULONG.to_string())
        .replace("{TAG_BYTE}", &tags::TAG_BYTE.to_string());
    let t = strings["true"];
    let f = strings["false"];
    let minus = strings["-"];
    let bool_to_string = format!(
        "(func $bool_to_string (param $v i32) (result i32)\n  local.get $v\n  (if (result i32)\n    (then i32.const {t})\n    (else i32.const {f})))\n"
    );
    let format = RUNTIME_FORMAT
        .replace("{minus}", &minus.to_string())
        .replace("{TAG_STRING}", &tags::TAG_STRING.to_string());
    format!("{object}\n{bool_to_string}\n{format}\n")
}

/// The heap starts (8-byte aligned) above the interned string segment, never below the string base.
/// Each interned string's mapped address points at its length word; its block extends `4 + len`
/// bytes beyond that (the length prefix + utf8, no NUL terminator).
pub(super) fn heap_base(strings: &IndexMap<String, u32>) -> u32 {
    let end = strings
        .iter()
        .map(|(s, addr)| addr + 4 + s.len() as u32)
        .max()
        .unwrap_or(STRING_BASE);
    (end.max(STRING_BASE) + 7) & !7
}

/// Reserves permanent static storage for every value-struct/value-union module global, starting at
/// `start` (typically past the interned-string region). Returns `(global_id → address, end)` where
/// `end` is 8-byte-aligned past the last slot (suitable as the itable base). Memory is assumed
/// zero-filled; `$__dream_init` constructs into these addresses.
pub(super) fn value_global_addrs(
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    start: u32,
) -> (HashMap<u32, u32>, u32) {
    let mut addr = start;
    let mut map = HashMap::new();
    for g in &mir.globals {
        if !interner.is_value_type(g.ty) {
            continue;
        }
        let (size, align) = interner.value_layout(g.ty).unwrap_or((4, 4));
        let align = align.max(1);
        addr = (addr + align - 1) & !(align - 1);
        map.insert(g.id.0, addr);
        addr = addr.saturating_add(size);
    }
    let end = (addr + 7) & !7;
    (map, end)
}
