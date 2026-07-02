# Classes & Structs

Dream provides two ways to group related data into object types: **Classes** and **Structs**. They share identical features (fields, constructors, methods, generics, properties, indexers, interfaces) but differ fundamentally in **how they are stored and copied**.

## Classes (Reference Types)

A `class` is a **reference type**. Instances live on the heap, and variables hold a *reference* to the instance. Assigning or passing a class shares the same object.

```dream
class Point {
    x: int;
    y: int;

    // Optional: a constructor to initialize fields
    constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }
}
```

### Creating and using classes

```dream
let p1 = Point(3, 4);
let p2 = p1;    // Shares the same object!
p2.x = 10;
println(p1.x);  // 10
```

Classes are garbage-collected via automatic reference counting (ARC). You do not need to manually free them. If you define a `del()` destructor method, it will automatically run right before the object is destroyed.

## Structs (Value Types)

A `struct` is a **value type**. Instances are stored *inline* (on the stack, inside arrays, or inside other objects), and every assignment or argument pass makes an independent **copy**.

```dream
struct Vec2 {
    public x: int;
    public y: int;

    constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }
}
```

### Copy semantics

```dream
let v1 = Vec2(3, 4);
let v2 = v1;    // Makes a full copy!
v2.x = 10;
println(v1.x);  // 3 (unaffected)
```

Structs do not use heap allocation and have zero garbage collection overhead. However, they cannot be nullable (`Vec2?` is invalid), cannot recursively contain themselves by value, and cannot currently be dynamically upcast to a bare interface or `object` variable without boxing limitations.

## Common Features

Both classes and structs support the following features:

### Visibility
Members are **private by default**. Mark them `public` to allow outside access.

### Methods
Define methods using `fun`. They automatically receive a `this` parameter.

```dream
class Counter {
    count: int;
    public fun increment(): void { this.count = this.count + 1; }
}
```

### Properties (`get` / `set`)
You can define computed properties using `get` and `set` accessors.

```dream
class Config {
    public get version(): int { return 1; }
}
```

### Indexers & Enumerators
You can opt into `obj[i]` syntax by defining `get(index)` and `set(index, value)` methods. You can opt into `for (let x in obj)` loops by defining `iterator()` and `next()` methods.

## When to use which?

*   Use a **`struct`** for small, copyable bundles of data with value identity (points, vectors, colors, ranges).
*   Use a **`class`** when instances have a lifetime and identity that should be *shared* rather than copied (graph nodes, file handles, services).
