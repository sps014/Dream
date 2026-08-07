# Regex

**Package:** `system.text` — `import system.text;`

Pure-Dream regex engine (no host `RegExp`). Synchronous on every host.

```dream
import system;
import system.text;

fun main(): void {
    let digits = Regex("\\d+", "g");
    System.println(digits.test("abc123"));           // true
    System.println(digits.replace("a1b2c3", "#"));   // a#b#c#
    System.println(digits.match("a1b2c3").length);   // 3
}
```

## Flags

| Flag | Meaning |
| --- | --- |
| `g` | global — `replace` all matches; `match` returns every match |
| `i` | case-insensitive |
| `m` | multi-line (`^`/`$` at line boundaries) |
| `s` | dot-all (`.` matches newlines) |

## Methods

#### `Regex(pattern: string, flags: string)`

Compiles a pattern with optional flag letters (`g`, `i`, `m`, `s`). Compile once and reuse — compilation is not free.

```dream
let re = Regex("\\w+", "gi");
```

#### `test(input: string): bool`

Returns whether the pattern matches anywhere in `input`. Fast yes/no check — use before `match` when you only need presence.

```dream
System.println(Regex("\\d+", "").test("abc123"));  // true
```

#### `replace(input: string, replacement: string): string`

Returns `input` with matches replaced by `replacement`. With `g`, replaces every match; supports `$1`..`$9`, `$&`, and `$$` in the replacement string.

```dream
System.println(Regex("(\\d+)", "g").replace("a1b2", "[$1]"));  // a[1]b[2]
```

#### `match(input: string): string[]`

Returns match results as a string array. With `g`: every full match; without `g`: full match plus capture groups (`""` for non-participating groups).

```dream
let all = Regex("\\d+", "g").match("a1b2c3");       // ["1", "2", "3"]
let caps = Regex("(\\d{4})-(\\d{2})", "").match("2026-06");
System.println(caps[1]);  // 2026
```

## Supported syntax

| Feature | Syntax |
| --- | --- |
| Literals, any character | `a`, `.` (respects `s`) |
| Anchors | `^`, `$` (respect `m`), `\b`, `\B` |
| Quantifiers | `*`, `+`, `?`, `{m}`, `{m,}`, `{m,n}`, lazy forms |
| Alternation | `a\|b` |
| Groups | `(...)`, `(?:...)` |
| Character classes | `[abc]`, `[^abc]`, `[a-z]` |
| Shorthands | `\d`, `\D`, `\w`, `\W`, `\s`, `\S` |
| Escapes | `\n`, `\t`, `\r`, `\` before metacharacters |

**Not supported:** lookaround, backreferences (`\1`), named groups, `\p{...}`. Unsupported constructs parse best-effort without their special meaning.

A runnable example: [`sample/interop/regex.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/regex.dream).
