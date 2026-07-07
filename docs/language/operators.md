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

These work on `int`: `&` (and), `|` (or), `^` (xor), `<<` (shift left), `>>` (arithmetic shift right).

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
| unary | unary `-`, `!` |
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
