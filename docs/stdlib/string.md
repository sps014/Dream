# Strings

`string` is a built-in reference type: heap-allocated, length-prefixed UTF-8, so `size()` is O(1). It is available in every program with no import, and every method below works on any string value.

Build strings with `+` concatenation or [interpolation](../language/operators.md#string-interpolation) (`$"hi {name}"`).

## Length and access

`size()` returns the character count; `is_empty()` is `true` when there are none. Index with `s[i]` (read-only) or `char_at(i)` to get a `char`, and iterate with `for (let c in s)`:

```dream
let s = "abc";
println(s.size());      // 3
println(s[0]);          // 'a'
println(s.char_at(1));  // 'b'

for (let c in s) {
    println(c);         // 'a', 'b', 'c'
}
```

Indexing is read-only (no `s[i] = c`). Build derived strings with `substring`, `+`, or the low-level `String.alloc`/`String.set` helpers.

!!! note
    `char_at` and `s[i]` do no bounds checking.

## Searching

- `contains(sub)` — `true` if `sub` occurs anywhere (the empty string always does).
- `starts_with(prefix)` / `ends_with(suffix)` — prefix/suffix tests.
- `index_of(target)` — index of the first occurrence of a character as an `Option<int>`; `None` if absent.

```dream
println("hello world".contains("world"));         // true
println("hello".starts_with("hel"));              // true
let i = "hello".index_of('l').unwrap_or(0 - 1);   // 2
let j = "hello".index_of('z').unwrap_or(0 - 1);   // -1 (absent)
```

## Transforming

Each of these returns a **new** string:

- `substring(start, end)` — the half-open range `[start, end)`; a non-positive length yields `""`.
- `to_lower()` / `to_upper()` — ASCII case conversion.
- `trim()` — remove leading and trailing ASCII whitespace.
- `repeat(times)` — the string repeated; `0` or less yields `""`.

```dream
println("hello world".substring(6, 11));   // "world"
println("Hello World".to_lower());         // "hello world"
println("  hello  ".trim());               // "hello"
println("ab".repeat(3));                   // "ababab"
```

## Comparison

`equals(other)` returns `true` when the contents match — identical to `==`, which compares string contents (not addresses):

```dream
println("hello".equals("hello"));   // true
println("hello" == "hello");        // true
```
