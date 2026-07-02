# Interfaces

An **interface** is a contract: a named set of method signatures that a class promises to provide.
A value typed as an interface can hold *any* class that implements it, and method calls on that
value dispatch to the concrete class's implementation at runtime (polymorphism).

## Declaring an interface

An interface lists method signatures — a return type and parameters, but **no body**. Each signature
ends with a semicolon:

```dream
interface Animal {
    fun speak(): string;
    fun legs(): int;
}
```

Interfaces declare methods only. They cannot have fields, and (for now) cannot provide default method
bodies.

## Implementing an interface

A class implements one or more interfaces by listing them after a colon. It must define every method
of each interface with a matching signature:

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

A class can implement several interfaces at once:

```dream
class Robot : Animal, Serializable {
    // ... must implement every method of both Animal and Serializable
}
```

If a class declares `: Animal` but omits one of the interface's methods (or declares it with the
wrong signature), compilation fails with a clear error.

## Using an interface-typed value

A class value is accepted anywhere its interface is expected — this implicit **upcast** needs no
cast. The static type becomes the interface, but the value still remembers its concrete class:

```dream
fun describe(a: Animal): void {
    println(a.speak());   // dispatches to Cat.speak or Dog.speak at runtime
    println(a.legs());
}

fun main(): void {
    describe(Cat());      // meow / 4
    describe(Dog());      // woof / 2
}
```

You can also store an interface value explicitly, with or without a cast:

```dream
let c = Cat();
let a: Animal = c;          // implicit upcast
let b = (Animal)c;          // explicit upcast — same value
```

An interface value is just the underlying object, so upcasts and downcasts are free (no copying).

## Interfaces cannot be instantiated

An interface is an abstract contract, not a concrete type — calling it like a constructor is an error:

```dream
let a = Animal();   // error: cannot instantiate interface 'Animal'
```

## Checking the concrete type with `is`

Use `is` to test what an interface value (or an `object`) actually holds at runtime:

```dream
let a: Animal = Cat();
if (a is Cat) {
    println("it's a cat");
}
```

### `is`-with-binding

`is` can bind a new, narrowed local in one step: `expr is Type name` introduces `name: Type` scoped
to the branch guarded by the check, so you don't need a separate cast. It works for **any** target
type:

```dream
let a: Animal = Cat();
if (a is Cat cat) {
    // `cat` is a Cat here, aliasing the same object
    println(cat.speak());
}
```

It also works for value types held in an `object`, unboxing automatically:

```dream
fun show(o: object): void {
    if (o is int n) {
        println(n + 1);   // `n` is an int, unboxed from `o`
    }
}
```

The bound name exists **only** inside the taken branch — it is not visible in the `else` branch or
after the `if`.

!!! note
    `is`-with-binding is supported in `if (...)` conditions. Binding inside `&&` chains or `while`
    conditions is not yet supported.

## Generic interfaces

An interface can be generic, declaring type parameters that its methods use:

```dream
interface Container<T> {
    fun get(): T;
    fun size(): int;
}
```

A class — generic or not — implements a concrete or generic instance of it. When a generic class
implements a generic interface, its type parameter flows into the interface:

```dream
class Box<T> : Container<T> {
    public value: T;
    public fun get(): T { return this.value; }
    public fun size(): int { return 1; }
}
```

Each concrete use is **monomorphized**: `Box<int>` implements `Container<int>`, `Box<string>`
implements `Container<string>`, and so on — each gets its own itable, exactly like generic classes.
Dispatch then works through the monomorphized interface type:

```dream
fun describe(c: Container<int>): void {
    println(c.get());
    println(c.size());
}

fun main(): void {
    let b = Box<int>(7);
    describe(b);              // implicit upcast Box<int> -> Container<int>

    let c: Container<int> = b;   // implicit upcast via annotation
    println(c.get());            // dispatches to Box<int>.get

    let d = (Container<int>)b;   // explicit upcast to a generic interface
    println(d.get());
}
```

## Async interface methods

An interface method may be `async`. Calling it through an interface-typed receiver dispatches
dynamically to the concrete async implementation, which returns a `Future<T>` you `await`:

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
    let v = await f.fetch();   // dynamic dispatch; result is a Future<int> to await
    println(v);
}
```

An `async` interface method must be implemented by an `async` method (and a non-async method by a
non-async one) — the two compile to different shapes (a `Future`-producing constructor vs. a plain
call), so a mismatch is a compile error.

## How dispatch works

Interface calls use **tag-indexed itables** — the same idea as the JVM's `invokeinterface`. Every
object carries a runtime tag (its concrete class id) in its heap header. For each interface, the
compiler builds a compact table, indexed by that tag, of the concrete method implementations. A call
like `a.speak()` reads the object's tag, looks up the right function in the interface's table, and
calls it indirectly. Because Dream compiles the whole program at once, these tables are computed
entirely at compile time.

## Built-in `Equatable` and `Comparable`

Two generic interfaces are built into the prelude:

```dream
interface Equatable<T> { fun equals(other: T): bool; }
interface Comparable<T> { fun compare(other: T): int; }
```

A type implements them against itself (`class Money : Comparable<Money>, Equatable<Money>`). By
convention `compare` returns a negative number, zero, or a positive number when `this` is ordered
before, equal to, or after `other`.

- **`==` / `!=` route to `equals`.** When both operands are the same user type that implements
  `Equatable<Self>`, `a == b` lowers to `a.equals(b)` (and `a != b` to its negation). Primitives and
  strings keep their built-in equality. The ordering operators (`<`, `>`, `<=`, `>=`) are *not*
  overloaded — use `compare` directly for custom ordering.
- **`compare` powers sorting.** `List<T : Comparable<T>>.sort()` orders a list using `compare`, and
  `List<T>.sort_by(cmp)` takes an explicit comparator. See [List sorting](../stdlib/list.md#sorting).

Both interfaces work with [value structs](value-structs.md): when the concrete type is known (a direct
call or a [generic constraint](generics.md#generic-constraints)), dispatch is static with no boxing.

## Limits (current version)

- Interfaces declare method signatures only — no fields and no default method bodies.
- A [value struct](value-structs.md) can implement an interface and be dispatched statically, but it
  cannot yet be stored in a bare interface-typed variable (dynamic upcast / boxing is deferred).

## See also

- [Classes](classes.md) — defining types, methods, and visibility.
- [The `object` Type](objects.md) — the universal container and the `is` operator.
