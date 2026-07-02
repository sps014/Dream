# Primitives

Dream provides a standard set of primitive data types. Methods on primitive types are auto-imported and immediately available.

## Integers

Dream supports various sizes of signed and unsigned integers.

*   `int`: 32-bit signed integer (default for integer literals, e.g., `42`).
*   `uint`: 32-bit unsigned integer (suffix `u`, e.g., `42u`).
*   `long`: 64-bit signed integer (suffix `L`, e.g., `42L`).
*   `ulong`: 64-bit unsigned integer (suffix `uL`, e.g., `42uL`).
*   `byte`: 8-bit unsigned integer (suffix `b`, e.g., `255b`).

### Common Integer Methods
*   **`.min(other) / .max(other)`**: Returns the smaller / larger of the two values.
*   **`.clamp(lo, hi)`**: Constrains the value to the inclusive range `[lo, hi]`.
*   **`.abs()`**: Returns the absolute value (signed types only).
*   **`.signum()`**: Returns `-1` for negative, `0` for zero, `1` for positive (signed types only).
*   **`Type.parse(str)`**: Static method that parses a string into the corresponding integer type, returning a `Result<Type, string>`.

```dream
println(15.clamp(0, 10)); // 10
println((-5).abs());      // 5
let n = int.parse("42").unwrap_or(0); // 42
```

## Floating-Point Numbers

Dream supports 32-bit and 64-bit IEEE 754 floating-point numbers.

*   `float`: 32-bit floating-point number (suffix `f`, e.g., `3.14f`).
*   `double`: 64-bit floating-point number (suffix `d` or decimal point without suffix, e.g., `3.14` or `3.14d`).

### Common Float Methods
*   **`.abs()`**: Returns the absolute value.
*   **`.min(other) / .max(other)`**: Returns the smaller / larger of the two values.
*   **`double.parse(str)`**: Static method that parses a string into a double, returning a `Result<double, string>`.

## Booleans

The `bool` type represents a boolean value (`true` or `false`).

### Common Boolean Methods
*   **`.to_int()`**: Returns `1` for `true` and `0` for `false`.

```dream
println(true.to_int()); // 1
```

## Characters

The `char` type represents a single character (one code point stored as an `i32`). Write `char` literals in single quotes: `'A'`, `'\n'`.

### Common Character Methods
*   **`.is_digit()` / `.is_alpha()` / `.is_whitespace()`**: Check character properties.
*   **`.to_lower()` / `.to_upper()`**: Convert ASCII case.
*   **`.to_int()`**: Returns the numeric code point of the character.
*   **`.as_string()`**: Returns a new single-character string.

```dream
println('A'.is_alpha()); // true
println('A'.to_lower()); // 'a'
let s = 'H'.as_string(); // "H"
```
