# Classes & Structs

Classes and structs both group related data with fields, constructors, and methods. They share every feature — the one difference is **how they are stored and copied**: a `class` is a reference type, a `struct` is a value type.

## Classes are reference types

A `class` lives on the heap, and a variable holds a *reference* to it. Assigning or passing a class shares the same object:

```dream
class Point {
    x: int;
    y: int;

    constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }
}

let p1 = Point(3, 4);
let p2 = p1;    // shares the same object
p2.x = 10;
println(p1.x);  // 10
```

Classes are managed by automatic reference counting (ARC) — no manual frees. Define a `del()` destructor and it runs right before the object is destroyed. See [Memory Management](memory.md).

## Structs are value types

A `struct` is stored inline (on the stack, inside an array, or inside another object), and every assignment or argument pass makes an independent **copy**:

```dream
struct Vec2 {
    public x: int;
    public y: int;

    constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }
}

let v1 = Vec2(3, 4);
let v2 = v1;    // full copy
v2.x = 10;
println(v1.x);  // 3 (unaffected)
```

Structs need no heap allocation and have no GC overhead, so a struct held by value is never `null` and cannot recursively contain itself by value.

### When to use which

- Use a **`struct`** for small, copyable bundles with value identity — points, vectors, colors, ranges.
- Use a **`class`** when an instance has a lifetime and identity that should be *shared* rather than copied — graph nodes, file handles, services.

## Shared features

Both classes and structs support all of the following.

### Visibility

Members are **class-private by default** — reachable only from the type's own methods, regardless of file. Mark a member `public` to expose it. Separately, the type itself is **file-private by default** and needs `public` to be used from another file. See [Imports > Visibility](imports.md#visibility) for how the two axes combine.

### Methods

Declare methods with `fun`; each receives an implicit `this`:

```dream
class Counter {
    count: int;
    public fun increment(): void { this.count = this.count + 1; }
}
```

### Properties

Define computed properties with `get` / `set` accessors:

```dream
class Config {
    public get version(): int { return 1; }
}
```

### Indexers and enumerators

Opt into `obj[i]` syntax by defining `get(index)` and `set(index, value)`. Opt into `for (let x in obj)` loops by defining `iterator()` (returning an object with `next(): Option<T>`).

## Advanced: sealed types

Prefix a `class`, `struct`, or `enum` with `sealed` to forbid `extend` blocks from adding methods, locking the method surface to what the type declares:

```dream
sealed class Token { public kind: int; }

// error: Cannot extend sealed type 'Token'
extend Token { public fun describe(): string { return "token"; } }
```

`sealed` combines with `public` in either order (`public sealed class ...`). It only blocks user `extend` blocks — a sealed type may still implement interfaces (including their defaults) and derive `@json`.

## Advanced: boxing a struct

When a struct is used where a reference is expected, it is **boxed** into a heap copy. A nullable struct (`Vec2?`) stores a nullable pointer to a boxed value — so `null` is representable and `??` unboxes it back to an inline struct — and assigning a struct to a bare interface or `object` variable boxes it for dynamic dispatch.
