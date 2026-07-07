# Enums & Unions

`enum` covers two related ideas: a plain enum is a set of named integer constants, and a *discriminated union* is an enum whose variants carry typed data. You take unions apart with a pattern-matching `switch`.

## Enums

A plain `enum` defines named integer constants. Members number from `0`; an explicit value shifts the ones that follow:

```dream
enum Color { Red, Green, Blue }          // 0, 1, 2
enum Status { Active = 10, Inactive }    // 10, 11
```

Access a member with `Enum.Member`. Enum values are integers at runtime, so they interoperate with `int` and work as `switch` subjects and labels:

```dream
let c: Color = Color.Green;
println(c);              // 1
println(c.to_string());  // Green
```

## Discriminated unions

When **any** variant carries a payload `(...)`, the whole `enum` becomes a discriminated union. A value is exactly one variant, and each variant holds its own typed data:

```dream
enum Shape {
    Circle(radius: float),
    Rect(width: float, height: float),
    Empty,                       // a unit variant carries no data
}

let s = Shape.Circle(2.0);
let e = Shape.Empty;
```

### Pattern-matching switch

The pattern form of `switch` runs the first arm whose pattern fits and binds the payload. The variant qualifier is optional inside the arms because the subject type is known. It works in both expression and statement position:

```dream
// expression position: yields a value
let area = switch (s) {
    Circle(r)  => 3.14 * r * r,
    Rect(w, h) => w * h,
    Empty      => 0.0,
};

// statement position: arms may be blocks
switch (s) {
    Circle(r)  => { println(r); }
    Rect(w, h) => println(w * h),
    Empty      => println("empty"),
}
```

A pattern `switch` must be **exhaustive**. Cover every variant, or add a catch-all `_` (or binding) pattern:

```dream
switch (s) {
    Circle(r) => println("Circle"),
    _         => println("Other"),
}
```

### Advanced patterns

Patterns **nest** — a payload can be matched against a variant. Exhaustiveness is checked recursively, so covering every inner case covers the outer variant with no `_`:

```dream
enum Inner { A(v: int), B }
enum Outer { Wrap(inner: Inner), Bare }

switch (o) {
    Wrap(A(n)) => n,
    Wrap(B)    => -1,   // Wrap(A) + Wrap(B) together cover Wrap
    Bare       => 0,
}
```

Guards (`if <bool>`) narrow an arm further:

```dream
switch (opt) {
    Some(n) if n > 10 => println("big"),
    Some(n)           => println(n),
    None              => println("none"),
}
```

### Generic unions

Unions may be generic; the concrete type is inferred from constructor arguments, or supplied by annotation. Add methods with an `extend` block:

```dream
enum Option<T> { Some(value: T), None }
enum Result<T, E> { Ok(value: T), Err(error: E) }

let o  = Option.Some(42);         // inferred Option<int>
let n: Option<int> = Option.None; // annotation needed for the unit variant
```

### Value unions

Unions are heap-allocated and reference-counted by default. But if **every** variant's payload is a value type or primitive (`int`, `bool`, `float`, a value `struct`, ...), the union automatically becomes a **stack (value) union**: stored inline, copied by value, with zero heap allocation.

### JSON with `@json`

Mark a union `@json` to derive `to_json` / `from_json`. Each value serializes to an object tagged with a `"type"` key naming the active variant:

```dream
@json
enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }

let text = JSON.serialize(Shape.Circle(7));   // {"type":"Circle","radius":7}
```
