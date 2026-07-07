# Regex

`Regex` is a regular-expression class. Construct one with a pattern and flags, then `test`, `replace`, or `match`. Like [`File`](file.md) and [`HttpClient`](http.md), it is backed by host functions implemented once per runtime, so the same `.dream` runs unchanged everywhere. These calls are synchronous — no `await`.

## Runtime support

| Runtime | Regex engine |
| --- | --- |
| Wasmtime (native CLI) | Rust's [`regex`](https://docs.rs/regex) crate |
| Node.js / browser | JavaScript `RegExp` |

The API is identical across all three. One caveat: the native `regex`-crate engine has no lookaround or backreferences, so a pattern relying on those works under JS `RegExp` but falls back to a safe default natively (`test` returns `false`, `replace` returns the input unchanged, `match` returns an empty array). Stick to the common subset for portable patterns.

## Usage

```dream
fun main(): void {
    let digits = Regex("\\d+", "g");

    if (digits.test("abc123")) {
        System.println("has digits");
    }

    let cleaned = digits.replace("a1b2c3", "#");   // a#b#c#
    System.println(cleaned);

    let parts = digits.match("a1b2c3");            // ["1", "2", "3"]
    System.println(parts.size());                  // 3
}
```

## Flags

Flags are passed as a string, mirroring JavaScript:

| Flag | Meaning |
| --- | --- |
| `g` | global — `replace` affects every match, and `match` returns all matches |
| `i` | case-insensitive |
| `m` | multi-line (`^`/`$` match at line boundaries) |
| `s` | dot-all (`.` matches newlines) |

## Capture groups

Without the `g` flag, `match` returns the full match followed by each capture group (missing optional groups are `""`):

```dream
fun main(): void {
    let date = Regex("(\\d{4})-(\\d{2})", "i");
    let caps = date.match("2026-06");   // ["2026-06", "2026", "06"]
    System.println(caps[1]);            // 2026
}
```

## API reference

| Method | Description |
| --- | --- |
| `Regex(pattern, flags)` | construct from a pattern and a flags string |
| `test(input): bool` | true if `input` contains a match |
| `replace(input, replacement): string` | replace matches (`g` for all; `$1`/`${name}` group refs supported) |
| `match(input): string[]` | every match with `g`, else the full match + capture groups |

A runnable example lives in [`sample/interop/regex.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/regex.dream).
