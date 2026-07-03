# Operators

## Arithmetic

| Operator | Meaning         | Types              |
|----------|-----------------|--------------------|
| `+`      | Addition / string concat | `int`, `float`, `double`, `string` |
| `-`      | Subtraction     | `int`, `float`, `double` |
| `*`      | Multiplication  | `int`, `float`, `double` |
| `/`      | Division        | `int`, `float`, `double` |
| `%`      | Remainder       | `int`, `float`     |

Both operands must be the same type. Use an explicit cast if they differ:

```dream
let x = 7 / (float)2;   // 3.5
```

String concatenation uses `+`: when either operand is a `string`, the other is converted through
its [`to_string`](../stdlib/builtins.md) (a C-style enum renders its variant *name*, not the raw
discriminant), so any value can be appended without an explicit call:

```dream
let msg = "Hello, " + name + "!";
let line = "color = " + Color.Green;   // "color = Green"
```

## String interpolation

An interpolated string is written with a `$` prefix: `$"..."`. Any expression wrapped in `{ ... }` (a *hole*) is evaluated and spliced into the string. Non-string values are converted automatically through the [`to_string`](../stdlib/builtins.md) object protocol, exactly like the `+` form:

```dream
let name = "Ada";
let count = 3;
let msg = $"{name} has {count + 1} items";   // "Ada has 4 items"
```

Interpolation simply desugars to a `+` concatenation chain, so `$"{name} has {count + 1} items"` is equivalent to `"" + name + " has " + (count + 1) + " items"`.

Write a literal brace by doubling it: `{{` produces `{` and `}}` produces `}`.

```dream
let x = 5;
let s = $"{{literal}} and {x}";   // "{literal} and 5"
```

A hole cannot contain a string literal (the whole `$"..."` is a single token, so an inner `"` would end it). Build such strings with `+` instead.

## Comparison

All comparison operators return `bool`.

| Operator | Meaning                   |
|----------|---------------------------|
| `==`     | Equal                     |
| `!=`     | Not equal                 |
| `<`      | Less than                 |
| `<=`     | Less than or equal        |
| `>`      | Greater than              |
| `>=`     | Greater than or equal     |

String `==` and `!=` compare the **contents** of the strings, not their addresses.

## Logical

| Operator | Meaning     |
|----------|-------------|
| `&&`     | Logical AND |
| `\|\|`     | Logical OR  |
| `!`      | Logical NOT |

Both sides of `&&` and `||` must be `bool`. `&&` and `||` **short-circuit**: the right operand is only evaluated when it can still affect the result.

## Bitwise

These operate on `int` values:

| Operator | Meaning       |
|----------|---------------|
| `&`      | Bitwise AND   |
| `\|`      | Bitwise OR    |
| `^`      | Bitwise XOR   |
| `<<`     | Shift left    |
| `>>`     | Shift right (arithmetic) |

## Null-coalescing

`a ?? b` evaluates to `a` when it is non-null, otherwise to `b`. The left operand should be a nullable (`T?`) value and the result type is the unwrapped `T`:

```dream
let name: string? = lookup();
let display: string = name ?? "anonymous";
```

## Ternary

`cond ? a : b` evaluates `cond` (a `bool`); the result is `a` when true and `b` when false. Both branches must share a type:

```dream
let label = score >= 60 ? "pass" : "fail";
```

## Assignment

`=` assigns a new value to a variable, array element, or class field:

```dream
x = 10;
arr[0] = 99;
point.x = 3;
```

### Compound assignment and increment

`+=`, `-=`, `*=`, `/=`, and `%=` update a target in place, and `++`/`--` add or subtract one:

```dream
total += 5;     // total = total + 5
count++;        // count = count + 1
i--;
```

## Negation

Prefix `-` negates a numeric value:

```dream
let neg = -x;
```

## Operator precedence

Higher rows bind tighter:

| Precedence | Operators         |
|------------|-------------------|
| unary      | unary `-`, `!`    |
| highest    | `&`               |
|            | `^`               |
|            | `\|`               |
|            | `%`               |
|            | `*`, `/`          |
|            | `+`, `-`          |
|            | `<<`, `>>`        |
|            | `<`, `<=`, `>`, `>=`, `==`, `!=`, `is` |
|            | `&&`              |
|            | `\|\|`             |
| lowest     | `??`, then `? :`  |

Use parentheses to make order explicit.
