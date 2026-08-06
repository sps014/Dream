# Strings

**Package:** `system.text` — `import system.text;` (also pulled in by `import system;`)

`string` is a built-in reference type holding UTF-8 text. Basic operations (`+`, `size()`, `char_at`, interpolation) need no import. Higher-level helpers (`substring`, `split`, `trim`, `StringBuilder`, `Regex`, `Unicode`, …) require this package.

Build strings with `+` concatenation or [interpolation](../language/operators.md#string-interpolation) (`$"hi {name}"`).

## Length and access

`size()` returns the **Unicode scalar** (code point) count; `byte_size()` returns the UTF-8 byte length (O(1)). `is_empty()` is `true` when there are no scalars. Index with `s[i]` (read-only) or `char_at(i)` to get the `i`th scalar as a `char`; use `byte_at(i)` for raw UTF-8 byte access. Iterate with `for (let c in s)` — each `c` is one scalar:

```dream
import system;
import system.text;

let s = "aé🙂";
System.println(s.length);       // 3 (scalars)
System.println(s.byte_size());  // 7 (UTF-8 bytes)
System.println(s[0]);           // 'a'
System.println(s.char_at(1));   // 'é'
System.println(s.byte_at(1));   // 195 (second byte of é)

for (let c in s) {
    System.println(c);          // 'a', 'é', '🙂'
}
```

Indexing is read-only (no `s[i] = c`). Build derived strings with `substring`, `+`, or the low-level `string.alloc`/`string.set` helpers (scalar indices).

!!! note
    `char_at`, `s[i]`, and `byte_at` [panic](../language/panics.md) on an out-of-range (including negative) index.

## Searching

- `contains(sub)` — `true` if `sub` occurs anywhere (the empty string always does).
- `starts_with(prefix)` / `ends_with(suffix)` — prefix/suffix tests.
- `index_of(target)` — index of the first occurrence of a character as an `Option<int>`; `None` if absent. Overloaded for substring search: `index_of(sub: string)`.
- `split(sep)` — split on a `char` or `string` separator into `string[]`.
- `replace(old, replacement)` — replace every occurrence of substring `old`.

```dream
System.println("hello world".contains("world"));         // true
System.println("hello".starts_with("hel"));              // true
let i = "hello".index_of('l').unwrap_or(0 - 1);   // 2
let j = "hello".index_of('z').unwrap_or(0 - 1);   // -1 (absent)
```

## Transforming

Each of these returns a **new** string:

- `substring(start, end)` — the half-open scalar range `[start, end)`; a non-positive length yields `""`.
- `to_lower()` / `to_upper()` — ASCII case conversion.
- `to_lower_unicode()` — full Unicode lowercase (via `Unicode.to_lower_unicode`).
- `trim()` — remove leading and trailing ASCII whitespace.
- `repeat(times)` — the string repeated; `0` or less yields `""`.
- `normalize(form)` — Unicode normalization (`UnicodeNormForm.Nfc`, `Nfd`, `Nfkc`, `Nfkd`).
- `graphemes()` — split into user-perceived grapheme clusters (`string[]`).

```dream
System.println("hello world".substring(6, 11));   // "world"
System.println("Hello World".to_lower());         // "hello world"
System.println("  hello  ".trim());               // "hello"
System.println("ab".repeat(3));                   // "ababab"
System.println("Straße".to_lower_unicode());      // "straße"
```

## Comparison

`equals(other)` returns `true` when the contents match — identical to `==`, which compares string contents (not addresses):

```dream
System.println("hello".equals("hello"));   // true
System.println("hello" == "hello");        // true
```

## Unicode helpers: `Unicode`

The `Unicode` class (same `system.text` package) exposes normalization, grapheme segmentation, and full case folding via host-backed intrinsics:

```dream
import system.text;

let nfc = Unicode.normalize("e\u0301", UnicodeNormForm.Nfc);
let lower = Unicode.to_lower_unicode("İ");  // Turkish I with dot
let parts = Unicode.graphemes("👨‍👩‍👧");    // family emoji as graphemes
```

`string` also exposes `.normalize(form)`, `.to_lower_unicode()`, and `.graphemes()` as thin wrappers.

## Building strings incrementally: `StringBuilder`

`+` concatenation is fine for a handful of pieces, but each `+` allocates a new string and copies everything accumulated so far — building up a string across a loop with `s = s + piece;` costs O(n²) overall. `StringBuilder` (same `system.text` package) appends into a single growable buffer and produces the final string with one allocation:

```dream
let sb = StringBuilder();
sb.append("Hello, ");
sb.append("world");
sb.append_char('!');
System.println(sb.build());   // "Hello, world!"
```

Methods:

- `.append(text)` — append a string.
- `.append_char(c)` — append a single character.
- `.append_line(text)` — append a string followed by `\n`.
- `.length` / `.is_empty()` — character count so far.
- `.clear()` — remove everything appended, keeping the backing buffer for reuse.
- `.build()` — materialize the accumulated characters into a new string.

`StringBuilder` also overrides `to_string()`, so `System.print` / `System.println` / `+` / interpolation all use `build()`'s output automatically — `System.println(sb)` and `System.println(sb.build())` are equivalent.
