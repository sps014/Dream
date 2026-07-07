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
