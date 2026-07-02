# Enums & Unions

Dream supports both simple C-style enumerations and full discriminated unions (algebraic data types) with payload data.

## Enums

A plain `enum` defines a set of named integer constants. Members are numbered from `0` by default; an explicit value makes the following members continue from it:

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

## Discriminated Unions

When **any** variant of an enum carries a payload `(...)`, the whole `enum` becomes a *discriminated union*. A union value is exactly one of its variants, and each variant can hold its own typed data. You extract the data with a pattern-matching `switch`.

```dream
enum Shape {
    Circle(radius: float),
    Rect(width: float, height: float),
    Empty,                       // a unit variant carries no data
}
```

Construct a variant with member-access call syntax:

```dream
let s = Shape.Circle(2.0);
let e = Shape.Empty;
```

### Pattern-Matching `switch`

The pattern-matching form of `switch` inspects a union value and runs the first arm whose pattern fits. The variant qualifier is optional inside a pattern `switch` because the subject type is already known.

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

A pattern `switch` must be **exhaustive** (cover every possible variant). If you don't list all variants, you must provide a catch-all `_` or binding pattern.

```dream
switch (s) {
    Circle(r) => println("Circle"),
    _         => println("Other"),
}
```

You can also use guards (`if <bool>`) to narrow arms:

```dream
switch (opt) {
    Some(n) if n > 10 => println("big"),
    Some(n)           => println(n),
    None              => println("none"),
}
```

### Generics

Unions may be generic; the concrete type is inferred from the constructor arguments, or supplied by an annotation:

```dream
enum Option<T> { Some(value: T), None }
enum Result<T, E> { Ok(value: T), Err(error: E) }

let o  = Option.Some(42);            // inferred Option<int>
let n: Option<int> = Option.None;    // annotation needed for the unit variant
```

You can use an `extend` block to add methods to generic unions.

### Value Unions

By default, discriminated unions are heap-allocated and reference-counted. However, if **every** variant's payload is a value type or primitive (`int`, `bool`, `float`, value `struct`, etc.), the union automatically becomes a **stack (value) union**. It is stored inline and copied by value with zero heap allocation.

### JSON with `@json`

Mark a discriminated union `@json` to derive `to_json` / `from_json` converters. Each value serializes to an object tagged with a `"type"` key naming the active variant:

```dream
@json
enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }

let text = JSON.serialize(Shape.Circle(7));   // {"type":"Circle","radius":7}
```
