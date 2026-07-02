# Value Structs

A `struct` is a **value type**. It has all the features of a [class](classes.md) — fields, a
`constructor`, instance and static methods, generics, `get`/`set` properties, and indexers — but it
differs in one fundamental way: **how it is stored and copied**.

- A `class` is a **reference type**: an instance lives on the heap and variables hold a *reference*
  to it. Assigning or passing it shares the same object.
- A `struct` is a **value type**: an instance is stored *inline* (in a local, a field, an array
  element, or a union payload) and every assignment or argument pass makes an independent **copy**.

```dream
struct Vec2 {
    public x: int;
    public y: int;

    constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }

    public fun length_squared(): int {
        return this.x * this.x + this.y * this.y;
    }
}
```

## Copy semantics

Assigning a value struct copies it. Mutating the copy never affects the original:

```dream
fun main() {
    let a = Vec2(1, 2);
    let b = a;        // full copy
    b.x = 100;        // mutates b only

    println(a.x);     // 1
    println(b.x);     // 100
}
```

Passing a value struct to a function also copies it, so a function cannot mutate its caller's value
through a value parameter:

```dream
fun bump(v: Vec2): int {
    v.x = v.x + 1000;   // mutates the local copy
    return v.x;
}

fun main() {
    let a = Vec2(1, 2);
    println(bump(a));   // 1001
    println(a.x);       // 1  — the caller's value is untouched
}
```

Methods run on the value in place, so a method *can* mutate the variable it is called on (the
receiver `this` is not copied):

```dream
struct Counter {
    public n: int;
    public fun bump() { this.n = this.n + 1; }
}

fun main() {
    let c = Counter();
    c.bump();
    c.bump();
    println(c.n);   // 2
}
```

## Structs can hold references

A value struct may contain reference-typed fields (a `class`, `string`, array, `List<T>`, etc.).
Copying the struct copies the reference and keeps reference counting correct: the referenced object
is retained on copy and released when the struct goes out of scope or is overwritten. You never leak
and never double-free.

```dream
class Buffer {
    public data: string;
    constructor(data: string) { this.data = data; }
}

struct Wrapper {
    public buf: Buffer;      // a reference field, held by value inside the struct
    constructor(buf: Buffer) { this.buf = buf; }
}
```

## Where value structs live

A value struct is stored inline everywhere it appears, and participates in copy/retain/drop in each:

- **Locals** — stored in the function's frame.
- **Class fields** — embedded directly inside the heap object (no extra allocation).
- **Array elements** — the element stride is the struct's full size; `xs[i]` is an inline value.
- **Union / `Option` payloads** — `Option<Vec2>` stores the `Vec2` inline.

```dream
fun main() {
    let points = [Vec2(1, 2), Vec2(3, 4)];
    points[0].x = 100;          // mutates element 0 in place
    println(points[0].x);       // 100
    println(points[1].x);       // 3  — a separate element
}
```

## Implementing interfaces

A value struct may `implements` an interface, including the built-in [`Equatable`/`Comparable`](interfaces.md)
protocols. Because Dream monomorphizes generics, an interface method is dispatched **statically** with
**zero boxing** wherever the concrete type is known — a direct call, or inside generic code whose type
parameter is [constrained](generics.md#generic-constraints) to the interface.

```dream
struct Money : Comparable<Money>, Equatable<Money> {
    public cents: int;
    constructor(cents: int) { this.cents = cents; }

    public fun compare(other: Money): int { return this.cents - other.cents; }
    public fun equals(other: Money): bool { return this.cents == other.cents; }
}

fun main() {
    println(Money(100) == Money(100));   // true — `==` routes to `equals`

    let prices = List<Money>();
    prices.push(Money(300));
    prices.push(Money(100));
    prices.sort();                        // uses `compare`, dispatched statically (no allocation)
}
```

Storing a value struct in a bare interface-typed or `object` variable — a true *dynamic* upcast that
would heap-copy (box) the value — is not yet supported. Direct calls and generic constraints cover
`==`, `compare`, and `sort` without any boxing.

## When to use a `struct`

Reach for a `struct` when a type is a small, copyable bundle of data with value identity — points,
vectors, colors, sizes, ranges, money amounts. Use a `class` when instances have a lifetime and
identity that should be *shared* rather than copied (nodes in a graph, a file handle, a service).

## Restrictions (this version)

Value structs currently exclude features that are inherent to reference types:

- **No `T?` nullability** — a value struct has no null representation, so a field cannot be a
  nullable value struct.
- **No dynamic upcast (boxing)** — a value struct may `implements` an interface and be dispatched
  statically (see above), but it cannot be stored in a bare interface-typed or `object` variable,
  which would require boxing to a tagged heap object.
- **No self-containment by value** — a value struct cannot contain itself by value (that would need
  infinite storage). Break the cycle with a reference (`class`) field or an array.
