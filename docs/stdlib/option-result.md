# Option & Result

Two built-in generic unions handle absence and failure safely, without null. Both are imported into every program automatically. They are ordinary [discriminated unions](../language/enums-unions.md), so you take them apart with a pattern-matching `switch`.

## `Option<T>`

`Option<T>` represents a value that may be absent:

```dream
enum Option<T> { Some(value: T), None }
```

Prefer it over a nullable `T?` when absence is a meaningful part of the flow — a lookup that might find nothing — because it forces the caller to handle both cases.

```dream
let some = Option.Some(42);
let none: Option<int> = Option.None;

let val = switch (some) {
    Some(v) => v,
    None    => 0,
};
```

Helpers:

- `.is_some()` / `.is_none()` — which variant it is.
- `.unwrap_or(fallback)` — the contained value, or `fallback`.

```dream
println(some.unwrap_or(0));   // 42
```

## `Result<T, E>`

`Result<T, E>` is the outcome of an operation that can fail — either a success (`Ok`) or an error (`Err`):

```dream
enum Result<T, E> { Ok(value: T), Err(error: E) }
```

Returning a `Result` makes failure an explicit part of the signature:

```dream
fun safe_div(a: int, b: int): Result<int, string> {
    if (b == 0) return Result.Err("divide by zero");
    return Result.Ok(a / b);
}

switch (safe_div(10, 2)) {
    Ok(v)  => println(v),
    Err(e) => println(e),
}
```

Helpers:

- `.is_ok()` / `.is_err()` — which variant it is.
- `.unwrap_or(fallback)` — the success value, or `fallback`.

!!! note
    There are no panicking `unwrap()` methods, by design. Always supply a fallback or use `switch` to handle the empty/error case explicitly.

## `?` — try-propagation

Writing a `switch` (or a chain of `is_ok()`/`unwrap_or()` calls) at every fallible call site gets
noisy fast. The postfix `?` operator is sugar for exactly that pattern: `expr?` either yields the
success payload, or immediately `return`s the failure/absence variant from the *enclosing
function*.

```dream
fun half(n: int): Result<int, string> {
    if (n % 2 != 0) {
        return Result.Err("odd");
    }
    return Result.Ok(n / 2);
}

// Without `?`:
fun quarter_verbose(n: int): Result<int, string> {
    return switch (half(n)) {
        Err(e) => Result.Err(e),
        Ok(h)  => switch (half(h)) {
            Err(e) => Result.Err(e),
            Ok(q)  => Result.Ok(q),
        },
    };
}

// With `?`:
fun quarter(n: int): Result<int, string> {
    let h = half(n)?;
    return Result.Ok(half(h)?);
}
```

The same operator works on `Option<T>`, propagating `None`:

```dream
fun first_positive(xs: int[]): Option<int> {
    for (let x in xs) {
        if (x > 0) {
            return Option.Some(x);
        }
    }
    return Option.None;
}

fun describe_first_positive(xs: int[]): Option<string> {
    let v = first_positive(xs)?;   // returns `Option.None` immediately if there isn't one
    return Option.Some("first positive: " + v.to_string());
}
```

Rules:

- `expr?` requires `expr` to be a `Result<T, E>` or `Option<T>`.
- The enclosing function must itself return a matching wrapper: `Result<_, E>` with the *same* `E`
  for a `Result<T, E>?`, or `Option<_>` for an `Option<T>?`. Using `?` in a function with an
  incompatible (or missing) return type is a compile error, since there is nowhere for the
  propagated failure to go.
- Postfix `?` is preferred over ternary unless a matching `:` follows at the same nesting depth, so
  `half(n)? + 1` and `if (half(n)? > 0)` are try-propagation. Write `cond ? a : b` when you mean the
  ternary; parentheses around `(expr?)` are never required for ordinary postfix use.
