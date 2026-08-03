# Operators

This page covers the operators Dream provides, grouped by what they do, plus string interpolation and the precedence table at the end.

## Arithmetic

| Operator | Meaning | Types |
|----------|---------|-------|
| `+` | Addition / string concat | `int`, `float`, `double`, `string` |
| `-` | Subtraction | `int`, `float`, `double` |
| `*` | Multiplication | `int`, `float`, `double` |
| `/` | Division | `int`, `float`, `double` |
| `%` | Remainder | `int`, `float` |

Both operands must be the same type. Cast one if they differ:

```dream
let x = 7 / (float)2;   // 3.5
```

Prefix `-` negates a number: `let neg = -x;`.

Integer arithmetic (`+`, `-`, `*`, `<<`, and unary `-`) **wraps** on overflow rather than panicking
or widening — see [Primitives § Integer overflow](primitives.md#integer-overflow) for the full
policy and per-type wrap widths. `/` and `%` by zero panic instead of wrapping.

## String concatenation

When either side of `+` is a `string`, the other side is converted through its [`to_string`](../stdlib/builtins.md). A C-style enum renders its variant *name*, not the number:

```dream
let msg = "Hello, " + name + "!";
let line = "color = " + Color.Green;   // "color = Green"
```

## String interpolation

Prefix a string with `$` and wrap expressions in `{ ... }`. Each hole is evaluated and converted to a string, just like `+`:

```dream
let name = "Ada";
let count = 3;
let msg = $"{name} has {count + 1} items";   // "Ada has 4 items"
```

Interpolation desugars to a `+` chain, so the above equals `"" + name + " has " + (count + 1) + " items"`.

Double a brace to write it literally — `{{` produces `{`, `}}` produces `}`:

```dream
let x = 5;
let s = $"{{literal}} and {x}";   // "{literal} and 5"
```

A hole cannot contain a string literal (the inner `"` would end the string). Use `+` for those cases.

## Comparison

All comparisons return `bool`.

| Operator | Meaning |
|----------|---------|
| `==` | Equal |
| `!=` | Not equal |
| `<` `<=` `>` `>=` | Ordering |

String `==` and `!=` compare **contents**, not addresses.

## Logical

`&&` (and), `||` (or), and `!` (not) operate on `bool`. `&&` and `||` **short-circuit**: the right operand runs only when it can still change the result.

## Bitwise

`&` (and), `|` (or), `^` (xor), `<<` (shift left), `>>` (shift right), and prefix `~` (complement)
work on any integer type: `int`, `uint`, `long`, `ulong`, `byte`. Both operands of a binary bitwise
op must be the same type, same as arithmetic. `>>` is an *arithmetic* (sign-extending) shift on the
signed types (`int`, `long`) and a *logical* (zero-filling) shift on the unsigned types (`uint`,
`ulong`, `byte`).

```dream
let flags: uint = 6u;           // 0b0110
let masked = flags & 4u;        // 4u   (0b0100)
let shifted: byte = 200b >> 2b; // 50b, zero-filled
let inverted = ~5;              // -6 (two's complement)
let inverted_b: byte = ~5b;     // 250b (wraps within byte's 0..255 range)
```

Like arithmetic, `~` and the binary bitwise ops on `byte` wrap their result into `byte`'s `0..255`
range — see [Primitives § Integer overflow](primitives.md#integer-overflow).

## Null-coalescing and ternary

`a ?? b` yields `a` when it is non-null, otherwise `b`. The left side is a nullable `T?` and the result is the unwrapped `T`:

```dream
let name: string? = lookup();
let display: string = name ?? "anonymous";
```

`cond ? a : b` picks `a` when `cond` is true, else `b`. Both branches must share a type:

```dream
let label = score >= 60 ? "pass" : "fail";
```

## Try-propagation

`expr?` unwraps a `Result<T, E>`/`Option<T>`, or `return`s the failure/absence variant from the
enclosing function immediately. See [Option & Result](../stdlib/option-result.md#try-propagation)
for the full rules.

```dream
fun quarter(n: int): Result<int, string> {
    let h = half(n)?;
    return Result.Ok(half(h)?);
}
```

A bare `expr?` immediately followed by a token that could start an expression (like `+`) is parsed
as the ternary's leading `cond ?`, not try-propagation; parenthesize it (`(half(n)?) + 1`) to
disambiguate.

## Assignment

`=` writes to a variable, array element, or field:

```dream
x = 10;
arr[0] = 99;
point.x = 3;
```

Compound forms update in place, and `++`/`--` step by one:

```dream
total += 5;   // total = total + 5
count++;
i--;
```

## Precedence

Higher rows bind tighter; use parentheses when in doubt.

| Precedence | Operators |
|------------|-----------|
| postfix | `?` (try-propagation) |
| unary | unary `-`, `!`, `~` |
| highest | `&` |
| | `^` |
| | `\|` |
| | `%` |
| | `*`, `/` |
| | `+`, `-` |
| | `<<`, `>>` |
| | `<`, `<=`, `>`, `>=`, `==`, `!=`, `is` |
| | `&&` |
| | `\|\|` |
| lowest | `??`, then `? :` |
