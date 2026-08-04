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

### Or-patterns and range patterns

An arm's pattern may be several alternatives separated by `|` — the arm runs if the subject matches any of them. Every alternative must be **binding-free** (a literal, a range, `_`, or a payload-free variant); an alternative that would bind a variable (`Circle(r)`, a bare name) is rejected, since which alternative matched isn't visible to pick a binding from:

```dream
switch (c) {
    'a' | 'e' | 'i' | 'o' | 'u' => println("vowel"),
    _                           => println("consonant"),
}

switch (shape) {
    Square | Triangle | Empty => println("no curves"),
    Circle(_)                 => println("curved"),
}
```

A literal pattern over an ordered scalar subject (`int`/`long`/`uint`/`ulong`/`byte`/`char`/`float`/`double`) can be an inclusive range, `lo..hi`:

```dream
fun grade(score: int): string {
    return switch (score) {
        90..100 => "A",
        80..89  => "B",
        70..79  => "C",
        _       => "F",
    };
}
```

Both compose with exhaustiveness checking the same way a single pattern does — `Square | Triangle | Empty` together with `Circle(_)` covers every variant with no `_` needed, for example.

### Generic unions

Unions may be generic; the concrete type is inferred from constructor arguments, or supplied by annotation. Add methods with an `extend` block:

```dream
enum Option<T> { Some(value: T), None }
enum Result<T, E> { Ok(value: T), Err(error: E) }

let o  = Option.Some(42);         // inferred Option<int>
let n: Option<int> = Option.None; // annotation needed for the unit variant
```

### Value unions

Unions are heap-allocated and reference-counted by default. But if **every** variant's payload is a value type or primitive (`int`, `bool`, `float`, a value `struct`, ...), the union automatically becomes a **stack (value) union**: stored inline, copied by value, with zero heap allocation. This is decided per concrete instantiation, so `Option<int>` is a value union while `Option<string>` stays a heap union, even though they share one generic declaration.

#### `@stack`: a checked contract, plus one relaxation

`@stack` on a union declaration turns "should be a value union" from a best-effort inference into a checked contract: the compiler reports an error if the union doesn't qualify, instead of silently falling back to the heap. This catches a regression (e.g. someone later adds a second `string` payload) at the declaration site rather than as a silent performance cliff.

`@stack` also unlocks one relaxation the automatic inference doesn't apply on its own: a union may still go inline even with **one** reference-typed payload field (a `string`, a `class`, an array, ...) across all of its variants — that field is stored inline as a retained pointer, exactly like a reference field embedded in a value `struct` already is. A union with *two or more* reference-typed payload fields, or one that refers to itself, still cannot be stored inline (`@stack` reports an error explaining which field forces it onto the heap):

```dream
@stack
enum Outcome {
    Success(code: int),
    Failure(code: int, reason: string), // one reference field: string
}
```

This relaxation is opt-in via `@stack` rather than automatic, because some existing patterns (a `weak` field typed `Option<SomeClass>`, for instance) depend on `Option<T>` staying a heap reference whenever `T` itself is a reference type — see [Memory](memory.md).

### JSON with `@json`

Mark a union `@json` to derive `to_json` / `from_json`. Each value serializes to an object tagged with a `"type"` key naming the active variant:

```dream
@json
enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }

let text = JSON.serialize(Shape.Circle(7));   // {"type":"Circle","radius":7}
```
