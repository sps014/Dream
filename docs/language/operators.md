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

Postfix `?` wins over ternary unless a matching `:` follows at the same nesting depth
(`half(n)? + 1` is try-propagation; `cond ? a : b` is still the ternary).

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

## Operator overloading

A class or struct can give `+`, `-`, `*`, `/`, `%`, `&`, `|`, `^`, `<<`, `>>`, `==`, unary `-`, `!`,
`~`, and both implicit and explicit casts their own meaning by tagging a method with
`@operator("...")` or `@cast("implicit"|"explicit")`. This is the same `@name(args)` attribute
syntax used elsewhere in Dream (`@override`, `@json`).

```dream
class Vector2 {
    public x: int;
    public y: int;

    constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }

    @operator("+")
    fun add(other: Vector2): Vector2 {
        return Vector2(this.x + other.x, this.y + other.y);
    }

    @operator("-")
    fun sub(other: Vector2): Vector2 {
        return Vector2(this.x - other.x, this.y - other.y);
    }

    // A one-parameter method takes the binary `-`; a zero-parameter method (same symbol) takes
    // unary `-`. Arity — not a separate attribute — tells them apart.
    @operator("-")
    fun negate(): Vector2 {
        return Vector2(-this.x, -this.y);
    }

    @operator("==")
    fun equalsVec(other: Vector2): bool {
        return this.x == other.x && this.y == other.y;
    }
}

fun main(): void {
    let a = Vector2(1, 2);
    let b = Vector2(3, 4);
    let c = a + b;        // Vector2(4, 6), dispatches to `add`
    let d = -a;            // Vector2(-1, -2), dispatches to `negate`
    let same = a == a;     // true, dispatches to `equalsVec`
    let diff = a != b;     // true — `!=` reuses a registered `@operator("==")`, negated
}
```

Rules:

- A tagged method's own parameter list fixes the operator's arity: one parameter is a binary
  overload (the right-hand operand), zero parameters is a unary overload. `+`/`*`/`/`/`%`/the
  bitwise operators/`==` are binary-only; `!`/`~` are unary-only; `-` may be either (as `sub`/`negate`
  above).
- `!=` has no operator of its own — a registered `@operator("==")` also powers it, negated.
- `<`, `<=`, `>`, `>=` are **not** tagged individually. Implement `Comparable<Self>` (see
  [Interfaces § Built-in `Equatable` and `Comparable`](interfaces.md#built-in-equatable-and-comparable))
  instead; all four ordering operators dispatch to its single `compare` method.
- A type may declare at most one overload per operator symbol/arity, and at most one cast per
  target type.

### User-defined casts

`@cast("implicit")`/`@cast("explicit")` on a no-parameter method defines a conversion from the
declaring type to the method's return type:

```dream
class Meters {
    public value: float;

    constructor(value: float) {
        this.value = value;
    }

    // Explicit only: `(float)meters`, never inferred.
    @cast("explicit")
    fun toFloat(): float {
        return this.value;
    }
}

class Money {
    public cents: int;

    constructor(cents: int) {
        this.cents = cents;
    }

    // Implicit: assignable anywhere an `int` is expected, no cast syntax needed.
    @cast("implicit")
    fun toCents(): int {
        return this.cents;
    }
}

fun main(): void {
    let m = Meters(2.5);
    let f = (float)m;          // explicit cast required

    let money = Money(150);
    let cents: int = money;    // implicit conversion at a typed `let` binding
}
```

An explicit `(T)expr` cast accepts either `@cast("implicit")` or `@cast("explicit")` — implicit
conversions are always also spellable explicitly. Implicit conversions themselves are currently
recognized at typed `let x: T = expr;` bindings; elsewhere, cast explicitly.

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
