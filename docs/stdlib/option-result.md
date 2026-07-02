# Option & Result

Dream provides two essential built-in generic unions to handle absence and failure safely. Both are automatically imported into every program.

## `Option<T>`

`Option<T>` represents a value that may be absent. It is defined as:

```dream
enum Option<T> { Some(value: T), None }
```

Use `Option<T>` instead of nullable (`T?`) when the absence is a meaningful part of the data flow, such as a lookup that might find nothing. It forces the caller to explicitly handle both cases.

### Using Option

```dream
let some = Option.Some(42);
let none: Option<int> = Option.None;

// Handling via switch
let val = switch (some) {
    Some(v) => v,
    None    => 0,
};
```

### Option Helpers
*   **`.is_some()`**: Returns `true` if it's `Some`.
*   **`.is_none()`**: Returns `true` if it's `None`.
*   **`.unwrap_or(fallback: T)`**: Returns the contained value or `fallback`.

```dream
println(some.unwrap_or(0)); // 42
```

## `Result<T, E>`

`Result<T, E>` represents the outcome of an operation that can fail. It provides either a success value (`Ok`) or an error (`Err`):

```dream
enum Result<T, E> { Ok(value: T), Err(error: E) }
```

Returning a `Result` makes failure an explicit part of a function's type signature.

### Using Result

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

### Result Helpers
*   **`.is_ok()`**: Returns `true` if it's `Ok`.
*   **`.is_err()`**: Returns `true` if it's `Err`.
*   **`.unwrap_or(fallback: T)`**: Returns the success value or `fallback`.

There are no built-in panicking `unwrap()` methods on purpose — you must always provide a fallback or use `switch` to explicitly handle the empty/error cases.
