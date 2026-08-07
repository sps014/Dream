# Option & Result

**Package:** `system.core` (bootstrap — no import required)

Two built-in generic unions handle absence and failure without null. Take them apart with `switch`. Console snippets need `import system;`.

## `Option<T>`

```dream
enum Option<T> { Some(value: T), None }
```

```dream
let some = Option.Some(42);
let none: Option<int> = Option.None;

let val = switch (some) {
    Some(v) => v,
    None    => 0,
};
```

#### `is_some(): bool`

Returns whether the option holds a value. Use for guards before unwrapping or chaining.

```dream
System.println(some.is_some());  // true
```

#### `is_none(): bool`

Returns whether the option is empty. Equivalent to `!is_some()` but reads clearly in negative checks.

```dream
System.println(none.is_none());  // true
```

#### `unwrap_or(fallback: T): T`

Returns the inner value or `fallback` when `None`. The safe alternative to a panicking unwrap — always supply a default or use `switch`.

```dream
System.println(some.unwrap_or(0));   // 42
System.println(none.unwrap_or(0));   // 0
```

#### `map<U>(f: fun(T): U): Option<U>`

Transforms the inner value if present; propagates `None` otherwise. Use to project without nested `switch` blocks.

```dream
System.println(some.map((x: int): int => x + 1).unwrap_or(0));  // 43
System.println(none.map((x: int): int => x + 1).is_none());     // true
```

#### `and_then<U>(f: fun(T): Option<U>): Option<U>`

Chains a fallible step that itself returns `Option`. Prefer over nested `map` + `flatten` when the next step may fail.

```dream
fun half(n: int): Option<int> {
    if (n % 2 != 0) return Option.None;
    return Option.Some(n / 2);
}
System.println(Option.Some(4).and_then(half).unwrap_or(0));  // 2
```

#### `or(fallback: Option<T>): Option<T>`

Returns `this` if `Some`, otherwise `fallback`. Use to supply a secondary source when the primary is absent.

```dream
System.println(none.or(Option.Some(7)).unwrap_or(0));  // 7
System.println(some.or(Option.Some(7)).unwrap_or(0));  // 42
```

## `Result<T, E>`

```dream
enum Result<T, E> { Ok(value: T), Err(error: E) }
```

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

#### `is_ok(): bool`

Returns whether the result is success. Quick branch before unwrapping or logging errors.

```dream
System.println(safe_div(10, 2).is_ok());  // true
```

#### `is_err(): bool`

Returns whether the result is failure. Pair with `is_ok` — pick whichever reads better at the call site.

```dream
System.println(safe_div(10, 0).is_err());  // true
```

#### `unwrap_or(fallback: T): T`

Returns the success value or `fallback` on `Err`. Discards the error — use `switch` or `map_err` when you need the failure detail.

```dream
System.println(safe_div(10, 2).unwrap_or(-1));  // 5
System.println(safe_div(10, 0).unwrap_or(-1));  // -1
```

#### `map<U>(f: fun(T): U): Result<U, E>`

Transforms the success payload while preserving the error type. Use to adapt values without re-matching `Ok`/`Err`.

```dream
let r = safe_div(10, 2).map((n: int): int => n * 2);
System.println(r.unwrap_or(0));  // 10
```

#### `map_err<F>(f: fun(E): F): Result<T, F>`

Transforms the error payload while preserving success. Use to normalize error types at API boundaries.

```dream
let r = safe_div(10, 0).map_err((e: string): int => e.length);
```

#### `and_then<U>(f: fun(T): Result<U, E>): Result<U, E>`

Chains a fallible step that returns `Result` with the same error type. The `Result` analogue of `Option.and_then`.

```dream
fun double_ok(n: int): Result<int, string> {
    return Result.Ok(n * 2);
}
System.println(safe_div(10, 2).and_then(double_ok).unwrap_or(0));  // 10
```

!!! note
    There are no panicking `unwrap()` methods. Always supply a fallback or use `switch`.

## `?` — try-propagation

`expr?` yields the success payload, or immediately `return`s the failure/absence from the enclosing function.

```dream
fun half(n: int): Result<int, string> {
    if (n % 2 != 0) {
        return Result.Err("odd");
    }
    return Result.Ok(n / 2);
}

fun quarter(n: int): Result<int, string> {
    let h = half(n)?;
    return Result.Ok(half(h)?);
}
```

Works on `Option<T>` too (propagates `None`):

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
    let v = first_positive(xs)?;
    return Option.Some("first positive: " + v.to_string());
}
```

Rules:

- `expr?` requires `Result<T, E>` or `Option<T>`.
- The enclosing function must return a matching wrapper (`Result<_, E>` with the same `E`, or `Option<_>`).
- Prefer postfix `?` over ternary unless a matching `:` follows; write `cond ? a : b` for ternary.

## `Error`

Stdlib fallible APIs use `Result<T, E>` where `E` implements:

```dream
public interface Error {
    fun message(): string;
    fun code(): string;   // "ENOENT", "EPARSE", "HTTP_404", …
}
```

### `ParseError` (bootstrap)

```dream
let e = ParseError.invalid("bad number");
System.println(e.message());
System.println(e.code());
```

### `ArgError` (`system`)

```dream
import system;
switch (System.set_env("", "x")) {
    Ok(_) => {},
    Err(e) => System.println(e.message()),
}
```

### `IoError` (`system.io`)

Factories: `IoError.not_found(path)`, `permission_denied(path)`, `other(path, msg)`, `exists(path)`.

```dream
import system.io;
let e = IoError.not_found("missing.txt");
System.println(e.code());  // ENOENT
```

Concrete types also: `HttpError` (`system.net`). Prefer `e.message()` / `e.code()` at call sites.
