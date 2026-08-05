# Option & Result

**Package:** `system.core` (bootstrap — no import required)

Two built-in generic unions handle absence and failure safely, without null. They are ordinary [discriminated unions](../language/enums-unions.md), so you take them apart with a pattern-matching `switch`. Console snippets below also need `import system;`.

## `Option<T>`

`Option<T>` represents a value that may be absent:

```dream
enum Option<T> { Some(value: T), None }
```

Prefer it when absence is a meaningful part of the flow — a lookup that might find nothing — because it forces the caller to handle both cases. There is no nullable `T?` / `null` spelling; `Option<T>` / `None` is the only absence model.

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
- `.map(f)` / `.and_then(f)` / `.or(fallback)` — transform or chain without nested `switch`.

```dream
System.println(some.unwrap_or(0));   // 42
System.println(some.map((x: int): int => x + 1).unwrap_or(0));  // 43
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
    Ok(v)  => System.println(v),
    Err(e) => System.println(e),
}
```

Helpers:

- `.is_ok()` / `.is_err()` — which variant it is.
- `.unwrap_or(fallback)` — the success value, or `fallback`.
- `.map(f)` / `.map_err(f)` — transform the success or error payload.
- `.and_then(f)` — chains into another `Result` when `Ok`, otherwise preserves the error.

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


## `Error`

Stdlib fallible APIs use `Result<T, E>` where `E` implements the `Error` interface:

```dream
public interface Error {
    fun message(): string;
    fun code(): string;   // stable machine code: "ENOENT", "EPARSE", "HTTP_404", …
}
```

Concrete types: `ParseError` (bootstrap), `IoError` (`system.io`), `HttpError` (`system.net`), `ArgError` (`system`). Prefer `e.message()` / `e.code()` at call sites.

