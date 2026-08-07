# Strings

**Package:** `system.text` — `import system.text;` (also pulled in by `import system;`)

`string` is a built-in reference type holding UTF-8 text. Basic operations (`+`, `.length`, `char_at`, interpolation) need no import. Higher-level helpers require this package.

Build strings with `+` or [interpolation](../language/operators.md#string-interpolation) (`$"hi {name}"`).

```dream
import system;
import system.text;
```

## Length and access

#### `.length`

Returns the number of Unicode scalars (code points), not UTF-16 code units or byte length. Use for user-visible character counts.

```dream
System.println("aé🙂".length);  // 3
```

#### `byte_size(): int`

Returns the UTF-8 byte length in O(1). Use when sizing buffers, wire protocols, or file I/O — not for "how many characters" display.

```dream
System.println(("aé🙂").byte_size());  // 6
```

#### `is_empty(): bool`

Returns whether the string has zero scalars. Prefer over `length == 0` for readability.

```dream
System.println("".is_empty());  // true
```

#### `char_at(i: int): char` / `s[i]` / `get(i: int): char`

Returns the `i`th scalar. Panics if out of range — bounds-check first when the index comes from user input.

```dream
let s = "abc";
System.println(s.char_at(1));  // 'b'
System.println(s[0]);          // 'a'
```

#### `s[i] = c` / `set_at(i: int, value: char): void`

Mutates one scalar in place (all aliases share the buffer). Prefer `StringBuilder` when replacements may change UTF-8 width.

```dream
let s = "abc";
s[0] = 'A';
System.println(s);  // Abc
```

#### `byte_at(i: int): byte`

Returns the raw UTF-8 byte at index `i`. Low-level — use `char_at` for text; use this for binary-oriented string work.

```dream
System.println(("aé🙂").byte_at(1));  // 195 (second byte of é)
```

Raw UTF-8 byte. Panics if out of range.

#### `for (let c in s)` / `iterator()`

Iterates one scalar per step. Prefer over indexing when you need every character without manual bounds checks.

```dream
for (let c in "aé🙂") {
    System.println(c);  // 'a', 'é', '🙂'
}
```

#### `string.alloc(n: int): string` (static)

Allocates a mutable string of `n` scalars without initializing content. Low-level — prefer `StringBuilder` for normal text building.

```dream
let buf = string.alloc(3);
string.set(buf, 0, 'x');
```

## Searching

#### `contains(sub: string): bool`

Returns whether `sub` appears anywhere in the string. Empty `sub` always matches — guard against empty needles if that matters.

```dream
System.println("hello world".contains("world"));  // true
```

#### `starts_with(prefix: string): bool`

Returns whether the string begins with `prefix`. Use for protocol prefixes, file extensions, or command routing.

```dream
System.println("hello".starts_with("hel"));  // true
```

#### `ends_with(suffix: string): bool`

Returns whether the string ends with `suffix`. Handy for file-type checks and URL path suffixes.

```dream
System.println("hello".ends_with("lo"));  // true
```

#### `index_of(target: char): Option<int>`

Returns the index of the first occurrence of a scalar, or `None`. Prefer over `index_of(string)` when searching a single character.

```dream
System.println("hello".index_of('l').unwrap_or(0 - 1));  // 2
System.println("hello".index_of('z').unwrap_or(0 - 1));  // -1
```

#### `index_of(sub: string): Option<int>`

Returns the index of the first occurrence of a substring. Use for slicing after a delimiter or validating a prefix position.

```dream
System.println("hello".index_of("ll").unwrap_or(0 - 1));  // 2
```

#### `split(sep: char): string[]` / `split(sep: string): string[]`

Splits on a delimiter into a new array of substrings. Multi-char `sep` handles tokens like `"::"` that a char split cannot express.

```dream
let parts = "a,b,c".split(',');
System.println(parts.length);  // 3
let words = "one::two".split("::");
```

#### `replace(old: string, replacement: string): string`

Returns a new string with every occurrence of `old` replaced. Non-mutating — chain or assign the result.

```dream
System.println("a-b-c".replace("-", "_"));  // a_b_c
```

## Transforming

Each returns a **new** string.

#### `substring(start: int, end: int): string`

Returns scalars in the half-open range `[start, end)`. Non-positive length yields `""` — validate indices when slicing user input.

```dream
System.println("hello world".substring(6, 11));  // world
```

#### `to_lower(): string` / `to_upper(): string`

ASCII-only case conversion. Fast for identifiers and English text — use `to_lower_unicode` for full Unicode.

```dream
System.println("Hello".to_lower());  // hello
System.println("Hello".to_upper());  // HELLO
```

#### `to_lower_unicode(): string`

Full Unicode lowercase mapping. Use for locale-aware display and case-insensitive comparison of non-ASCII text.

```dream
System.println("Straße".to_lower_unicode());  // straße
```

#### `trim(): string`

Strips leading and trailing ASCII whitespace. Does not trim Unicode space categories beyond ASCII — normalize first if needed.

```dream
System.println("  hello  ".trim());  // hello
```

#### `repeat(times: int): string`

Repeats the string `times` times. `times <= 0` yields `""`.

```dream
System.println("ab".repeat(3));  // ababab
```

#### `normalize(form: UnicodeNormForm): string`

Applies a Unicode normalization form (NFC, NFD, etc.). Use before comparing or hashing text that may use composed vs decomposed sequences.

```dream
let nfc = "e\u0301".normalize(UnicodeNormForm.Nfc);
```

#### `graphemes(): string[]`

Splits into user-perceived grapheme clusters (e.g. emoji ZWJ sequences stay together). Prefer over scalar iteration for cursor/selection UX.

```dream
let parts = "👨‍👩‍👧".graphemes();
```

## Comparison

#### `equals(other: string): bool`

Content equality — same as `==`. Use either form; `equals` reads well in method chains.

```dream
System.println("hello".equals("hello"));  // true
System.println("hello" == "hello");       // true
```

#### `compare(other: string): int`

Lexicographic compare returning `< 0`, `0`, or `> 0`. Required for `Comparable<string>` and sort keys.

```dream
System.println("a".compare("b"));  // negative
System.println("a".compare("a"));  // 0
```

## Unicode helpers: `Unicode`

#### `Unicode.normalize(text, form): string`

Static wrapper around normalization — same as `text.normalize(form)` but callable without a string receiver.

```dream
let nfc = Unicode.normalize("e\u0301", UnicodeNormForm.Nfc);
```

#### `Unicode.to_lower_unicode(text): string`

Static full-Unicode lowercase. Use when normalizing arbitrary `text` variables in utility code.

```dream
let lower = Unicode.to_lower_unicode("İ");
```

#### `Unicode.graphemes(text): string[]`

Static grapheme split. Same as instance `graphemes()` — pick whichever reads cleaner at the call site.

```dream
let parts = Unicode.graphemes("👨‍👩‍👧");
```

`UnicodeNormForm`: `Nfc`, `Nfd`, `Nfkc`, `Nfkd`.

## `StringBuilder`

Avoid `s = s + piece` in loops (O(n²)). Append into one buffer, then `build()`.

#### `StringBuilder(capacity: int = 16)`

Creates an empty builder with optional initial capacity. Pre-size when you know approximate output length.

```dream
let sb = StringBuilder();
let big = StringBuilder(256);
```

#### `append(text: string): void`

Appends a string fragment. The primary building block — chain many appends before `build()`.

```dream
sb.append("Hello, ");
```

#### `append_char(c: char): void`

Appends a single scalar. Prefer over `append` with a one-char string when appending character by character.

```dream
sb.append_char('!');
```

#### `append_line(text: string): void`

Appends `text` followed by `\n`. Convenience for log lines and multi-line output.

```dream
sb.append_line("world");
```

#### `.length` / `is_empty(): bool`

Reports current built length and whether nothing has been appended yet.

```dream
System.println(sb.length);
System.println(sb.is_empty());
```

#### `clear(): void`

Empties the builder but keeps the backing buffer for reuse. Call between logical output batches.

```dream
sb.clear();
```

#### `build(): string` / `to_string()`

Materializes the accumulated text as an immutable string. `to_string()` (and `println(sb)`) delegate to `build()`.

```dream
System.println(sb.build());
System.println(sb);  // uses to_string() → build()
```
