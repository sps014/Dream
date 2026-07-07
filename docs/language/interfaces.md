# Interfaces

An interface is a contract — a named set of method signatures that a type promises to provide. A value typed as an interface can hold *any* type that implements it, and calls dispatch to the concrete implementation at runtime. That is how Dream does polymorphism.

## Declaring an interface

List method signatures (return type and parameters). A signature with no body ends in a semicolon. Interfaces declare methods only — no fields:

```dream
interface Animal {
    fun speak(): string;
    fun legs(): int;
}
```

## Implementing an interface

A class implements interfaces by listing them after a colon, defining every method with a matching signature:

```dream
class Cat : Animal {
    public fun speak(): string { return "meow"; }
    public fun legs(): int { return 4; }
}

class Dog : Animal {
    public fun speak(): string { return "woof"; }
    public fun legs(): int { return 2; }
}
```

A class can implement several at once (`class Robot : Animal, Serializable { ... }`). Omitting a method, or declaring it with the wrong signature, is a compile error.

## Using an interface-typed value

A class value is accepted wherever its interface is expected — this **upcast** is implicit and free. The static type becomes the interface, but the value remembers its concrete class:

```dream
fun describe(a: Animal): void {
    println(a.speak());   // dispatches to Cat.speak or Dog.speak at runtime
    println(a.legs());
}

describe(Cat());   // meow / 4
describe(Dog());   // woof / 2
```

You can store an interface value explicitly, with or without a cast:

```dream
let c = Cat();
let a: Animal = c;     // implicit upcast
let b = (Animal)c;     // explicit upcast — same value
```

An interface is an abstract contract, so instantiating it (`Animal()`) is an error.

## Default methods

A method may carry a **default body** that implementers inherit unless they override it. A default can call the interface's other methods on `this`, which still dispatch to the concrete type:

```dream
interface Greeter {
    fun name(): string;
    fun greet(): string {           // default body
        return "Hello, I am " + this.name();
    }
}

class Person : Greeter {
    public fun name(): string { return "Ada"; }
    // inherits the default greet()
}

class Robot : Greeter {
    public fun name(): string { return "R2"; }
    public fun greet(): string { return "BEEP " + this.name(); }   // overrides
}
```

## Checking the concrete type with `is`

Test what an interface value (or `object`) actually holds at runtime:

```dream
let a: Animal = Cat();
if (a is Cat) {
    println("it's a cat");
}
```

### `is` with binding

`is` can narrow and bind in one step — `expr is Type name` scopes `name: Type` to the guarded branch, with no separate cast. It works for any target type, unboxing value types held in an `object`:

```dream
let a: Animal = Cat();
if (a is Cat cat) {
    println(cat.speak());   // `cat` aliases the same object
}
```

The bound name exists only inside the taken branch.

!!! note
    `is`-with-binding works in `if (...)` and `while (...)` conditions, and through a top-level `&&` chain (`if (x is T t && cond)` makes `t` available in the branch body). It is not yet visible in *later conjuncts of the same condition* (e.g. `if (x is T t && t.ok())`) — reference it in the branch body instead.

## Advanced

### Implementing an interface with `extend`

An `extend` block can carry an `implements` clause, making an **existing** type — even a primitive — satisfy an interface:

```dream
extend int : Comparable<int> {
    public fun compare(other: int): int {
        if (this < other) { return 0 - 1; }
        if (this > other) { return 1; }
        return 0;
    }
}
```

This is exactly how the prelude makes primitives `Comparable` so `List<int>().sort()` works. The type then satisfies [generic constraints](generics.md#generic-constraints) like `T : Comparable<T>`.

### Generic interfaces

An interface can be generic. A class implements a concrete or generic instance of it; when a generic class implements a generic interface, its type parameter flows in:

```dream
interface Container<T> {
    fun get(): T;
    fun size(): int;
}

class Box<T> : Container<T> {
    public value: T;
    public fun get(): T { return this.value; }
    public fun size(): int { return 1; }
}
```

Each concrete use is monomorphized: `Box<int>` implements `Container<int>`, and gets its own itable. Dispatch works through the monomorphized interface type:

```dream
fun describe(c: Container<int>): void {
    println(c.get());
    println(c.size());
}

let b = Box<int>(7);
describe(b);                   // implicit upcast Box<int> -> Container<int>
let d = (Container<int>)b;     // explicit upcast to a generic interface
```

### Async interface methods

An interface method may be `async`. Calling it through an interface receiver dispatches dynamically to the concrete async implementation, which returns a `Future<T>` to `await`:

```dream
interface Fetcher {
    async fun fetch(): int;
}

class Remote : Fetcher {
    public base: int;
    public async fun fetch(): int {
        await Time.sleep(10);
        return this.base + 1;
    }
}

async fun run(f: Fetcher): void {
    let v = await f.fetch();   // dynamic dispatch; await the Future<int>
    println(v);
}
```

An `async` interface method must be implemented by an `async` method (and non-async by non-async) — the two compile to different shapes, so a mismatch is a compile error.

### How dispatch works

Interface calls use **tag-indexed itables**, like the JVM's `invokeinterface`. Every object carries a runtime tag (its concrete class id) in its heap header. For each interface, the compiler builds a compact table, indexed by that tag, of the concrete implementations. A call reads the tag, looks up the method, and calls it indirectly. Because Dream compiles the whole program at once, these tables are computed entirely at compile time.

### Built-in `Equatable` and `Comparable`

Two generic interfaces are built into the prelude:

```dream
interface Equatable<T> { fun equals(other: T): bool; }
interface Comparable<T> { fun compare(other: T): int; }
```

A type implements them against itself (`class Money : Comparable<Money>, Equatable<Money>`). By convention `compare` returns a negative number, zero, or a positive number when `this` is ordered before, equal to, or after `other`.

Every numeric primitive plus `char` and `string` already implements `Comparable` (via prelude `extend` blocks), so `List<int>().sort()`, `binary_search`, and comparisons in generic code work with no extra code.

- **`==` / `!=` route to `equals`** when both operands are the same user type implementing `Equatable<Self>`. Primitives and strings keep built-in equality. The ordering operators (`<`, `>`, `<=`, `>=`) are *not* overloaded — use `compare` for custom ordering.
- **`compare` powers sorting** via `List<T : Comparable<T>>.sort()` and `List<T>.sort_by(cmp)`. See [List sorting](../stdlib/collections.md#sorting).

Both interfaces work with [value structs](classes-structs.md): when the concrete type is known (a direct call or a generic constraint), dispatch is static with no boxing. Assigning a value struct to a bare interface variable boxes it into a tagged heap object, after which it dispatches dynamically:

```dream
let a: Shape = Rect(3, 4);   // boxed; a.area() dispatches dynamically
```

## See also

- [Classes & Structs](classes-structs.md) — defining types, methods, and visibility.
- [The `object` Type](objects.md) — the universal container and the `is` operator.
