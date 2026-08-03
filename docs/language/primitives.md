# Primitives

Primitive types are the built-in scalars: integers, floats, booleans, and characters. Their methods are auto-imported, so you can call them anywhere without an import. For the full type list and literal suffixes, see [Types](types.md).

## Integers

Signed and unsigned, in several widths:

- `int` — 32-bit signed (the default for integer literals: `42`).
- `uint` — 32-bit unsigned (`42u`).
- `long` — 64-bit signed (`42L`).
- `ulong` — 64-bit unsigned (`42uL`).
- `byte` — 8-bit unsigned (`255b`).

Common methods:

- `.min(other)` / `.max(other)` — the smaller / larger of two values.
- `.clamp(lo, hi)` — constrain to the inclusive range `[lo, hi]`.
- `.abs()` — absolute value (signed types only).
- `.signum()` — `-1`, `0`, or `1` by sign (signed types only).
- `Type.parse(str)` — static; parses a string into that integer type, returning `Result<Type, string>`.

```dream
println(15.clamp(0, 10));              // 10
println((-5).abs());                   // 5
let n = int.parse("42").unwrap_or(0);  // 42
```

### Integer overflow

Every integer primitive **wraps** on overflow: two's-complement modulo its own bit width, with no
trap and no promotion to a wider type. This matches WebAssembly's native `i32`/`i64` arithmetic
(the compiler's target), so wrapping is the zero-cost default rather than a runtime check.

- `int`/`uint` wrap at 32 bits, `long`/`ulong` at 64 bits, `byte` at 8 bits (`+`, `-`, `*`, `<<`
  all wrap; `byte` shares WASM's 32-bit `i32` register, so the compiler masks its result back into
  `[0, 255]` after every op instead of getting the narrower width for free).
- A binary op's result type is its **left operand's** type — `byte + byte` stays `byte`, it is
  never promoted to `int` the way C promotes narrow integer types.
- `/` and `%` by zero panic (a WASM integer division trap, routed through the same diagnosable
  panic path as other runtime checks) rather than wrapping.

```dream
let i: int = 2147483647;   // int.max
let j = i + 1;              // wraps to -2147483648, not a panic or a wider type

let b: byte = 250b;
let b2 = b + 10b;           // wraps to 4 (260 mod 256), stays byte
```

There is no `checked`/`saturating` arithmetic mode; use `.min`/`.max`/`.clamp()` above, or check
operands before an operation, if you need to guard against wraparound explicitly.

## Floating point

IEEE 754, in two widths:

- `float` — 32-bit (`3.14f`).
- `double` — 64-bit (`3.14` or `3.14d`).

Common methods:

- `.abs()` — absolute value.
- `.min(other)` / `.max(other)`.
- `double.parse(str)` — static; parses a string into a `double`, returning `Result<double, string>`.

## Booleans

`bool` is `true` or `false`.

- `.to_int()` — `1` for `true`, `0` for `false`.

```dream
println(true.to_int());   // 1
```

## Characters

`char` is a single character (one code point stored as an `i32`). Write literals in single quotes: `'A'`, `'\n'`.

- `.is_digit()` / `.is_alpha()` / `.is_whitespace()` — classify the character.
- `.to_lower()` / `.to_upper()` — ASCII case conversion.
- `.to_int()` — the numeric code point.
- `.as_string()` — a new single-character string.

```dream
println('A'.is_alpha());   // true
println('A'.to_lower());   // 'a'
let s = 'H'.as_string();   // "H"
```
