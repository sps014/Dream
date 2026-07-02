# string

`string` is a built-in reference type (heap-allocated, length-prefixed UTF-8, so `size()` is O(1)). It is available in every program with no import. These methods are available on any string value.

Strings can be built with `+` concatenation or [string interpolation](../language/operators.md#string-interpolation) (`$"hi {name}"`).

## size

Returns the number of characters. This is the same `size()` the built-in arrays and the stdlib
`List`/`Map` expose, so every collection is measured the same way:

```dream
let n = "hello".size();   // 5
```

## is_empty

Returns `true` when the string has no characters.

```dream
println("".is_empty());       // true
println("hi".is_empty());     // false
```

## char_at

Returns the character at `index`. No bounds checking.

```dream
let c = "hello".char_at(1);   // 'e'
```

## Indexing and iteration

A string is indexable and iterable, exactly like the built-in arrays and the stdlib `List`/`Map`.
`s[i]` reads the character at position `i` (a `char`, the same value as `char_at`), and
`for (let c in s)` walks its characters in order:

```dream
let s = "abc";
println(s[0]);          // 'a'

for (let c in s) {
    println(c);         // 'a', 'b', 'c'
}
```

Indexing is read-only (there is no `s[i] = c`); build derived strings with `substring`, `+`, or the
low-level `String.alloc`/`String.set` helpers instead.

## substring

Returns a new string containing the characters in the half-open range `[start, end)`. A non-positive length yields the empty string.

```dream
let s = "hello world".substring(6, 11);   // "world"
```

## index_of

Returns the index of the first occurrence of character `target` as an `Option<int>`: `Some(index)`, or `None` if absent. Use `unwrap_or` (or `switch`) to read it.

```dream
let i = "hello".index_of('l').unwrap_or(0 - 1);   // 2
let j = "hello".index_of('z').unwrap_or(0 - 1);   // -1 (absent)
```

## contains

Returns `true` if `sub` occurs anywhere in the string. The empty string is always contained.

```dream
println("hello world".contains("world"));   // true
println("hello world".contains("xyz"));     // false
```

## starts_with

Returns `true` if the string begins with `prefix`.

```dream
println("hello".starts_with("hel"));   // true
```

## ends_with

Returns `true` if the string ends with `suffix`.

```dream
println("hello".ends_with("llo"));   // true
```

## to_lower

Returns a new string with every ASCII uppercase letter lowercased.

```dream
println("Hello World".to_lower());   // "hello world"
```

## to_upper

Returns a new string with every ASCII lowercase letter uppercased.

```dream
println("Hello World".to_upper());   // "HELLO WORLD"
```

## trim

Returns a new string with leading and trailing ASCII whitespace removed.

```dream
println("  hello  ".trim());   // "hello"
```

## repeat

Returns a new string consisting of the original repeated `times` times. A count of `0` or less yields the empty string.

```dream
println("ab".repeat(3));   // "ababab"
```

## equals

Returns `true` if this string has the same contents as `other`. This is identical to using `==`.

```dream
println("hello".equals("hello"));   // true
```
