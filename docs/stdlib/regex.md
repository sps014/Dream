# Regex

`Regex` is a regular-expression class implemented entirely in Dream — a Thompson-NFA/Pike-VM
engine (the same family of algorithm as RE2, Go's `regexp`, and Rust's `regex` crate), not a
binding to a host library. Construct one with a pattern and flags, then `test`, `replace`, or
`match`. There is nothing runtime-specific about it: the exact same compiled code runs under
wasmtime, Node, and the browser, and these calls are synchronous — no `await`.

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

Without the `g` flag, `match` returns the full match followed by each capture group (a group that
didn't participate — e.g. inside an alternative that wasn't taken — is `""`):

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
| `replace(input, replacement): string` | replace matches (`g` for all; `$1`.."$9" and `$&` group refs supported, `$$` for a literal `$`) |
| `match(input): string[]` | every match with `g`, else the full match + capture groups |

## Supported syntax

| Feature | Syntax |
| --- | --- |
| Literals, any character | `a`, `.` (respects the `s` flag) |
| Anchors | `^`, `$` (respect the `m` flag), `\b`, `\B` |
| Quantifiers | `*`, `+`, `?`, `{m}`, `{m,}`, `{m,n}`, and their lazy forms (`*?`, `+?`, `??`, `{m,n}?`) |
| Alternation | `a\|b` |
| Groups | `(...)` (capturing), `(?:...)` (non-capturing) |
| Character classes | `[abc]`, `[^abc]`, `[a-z]` |
| Shorthand classes | `\d`, `\D`, `\w`, `\W`, `\s`, `\S` |
| Escapes | `\n`, `\t`, `\r`, and `\` before any metacharacter (`\.`, `\\`, `\(`, ...) |

**Not supported:** lookaround (`(?=...)`, `(?!...)`, ...), backreferences (`\1`), named groups
(`(?<name>...)`), and Unicode property classes (`\p{...}`) — the same NFA-based family of engine
this one belongs to (RE2, Go's `regexp`) leaves these out too, since they can't be matched in
guaranteed linear time. A pattern that uses one of these constructs won't crash; it just won't
carry that construct's special meaning (e.g. `(?=...)` parses as a best-effort non-capturing group,
`\1` as a literal `1`).

A runnable example lives in [`sample/interop/regex.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/regex.dream).
