# Generics

Generics let you write code once and use it for many types. Dream resolves them at compile time: for each concrete type you use, the compiler emits a separate, fully optimized copy. There is no boxing and no runtime cost.

## Generic functions

Add `<T>` after the function name. The type argument is usually inferred from the call, but explicit `<Type>` always works:

```dream
fun first<T>(arr: T[]): T {
    return arr[0];
}

let nums = [10, 20, 30];
let words = ["a", "b", "c"];
println(first<int>(nums));      // 10
println(first(words));          // "a" (inferred)
```

Multiple type parameters are allowed: `fun swap<A, B>(a: A, b: B): A { ... }`.

## Generic classes

Classes and structs can be generic. Type arguments can themselves be generic or arrays, so generics nest freely:

```dream
class Pair<A, B> {
    first: A;
    second: B;
}

let p = Pair<int, string>(1, "one");
println(p.first);   // 1

let nested = Pair<Box<int>, int>(Box<int>(7), 5);
println(nested.first.v);   // 7
```

## Generic methods

A method automatically sees its class's type parameters:

```dream
class Box<T> {
    value: T;
    fun get(): T { return this.value; }
    fun set(v: T): void { this.value = v; }
}

let b = Box<int>(42);
b.set(100);
println(b.get());   // 100
```

## Advanced

### Generic constraints

Constrain a type parameter to one or more interfaces with `T : Iface`. Inside the body, the constrained parameter exposes that interface's methods. Constraints apply to functions, classes, structs, interfaces, and `extend` blocks:

```dream
fun max_of<T : Comparable<T>>(a: T, b: T): T {
    if (a.compare(b) > 0) { return a; }   // compare available because T : Comparable<T>
    return b;
}
```

Combine bounds with `+`:

```dream
struct Sorted<T : Comparable<T> + Equatable<T>> { /* ... */ }
```

At each instantiation the compiler checks the concrete type satisfies the constraint, reporting an error otherwise (e.g. `List<int>().sort()` needs `int : Comparable<int>`). Because generics are monomorphized, a constrained call binds to the concrete method with **static dispatch and no boxing** — even for [value structs](classes-structs.md).

### Static methods on a generic class

Dispatch a `static` method by naming the class with concrete arguments on the receiver, `Class<Args>.method(...)`. The compiler monomorphizes the class for those arguments:

```dream
class Cache<T> {
    seed: int;
    constructor(seed: int) { this.seed = seed; }
    public static fun make(seed: int): Cache<T> {
        return Cache<T>(seed);
    }
}

let c = Cache<int>.make(5);   // monomorphizes Cache<int>, calls Cache<int>.make
println(c.seed);              // 5
```

As with any static member, the method must be `public` to be called from another file, and the generic class itself must be `public` to be referenced across files. See [visibility](imports.md).

### Generic functions as first-class values

A generic function can become a [first-class function value](functions.md). Its type arguments are inferred from the expected function type at the use site, then monomorphized like any other instance:

```dream
fun natural_order<T : Comparable<T>>(a: T, b: T): int {
    return a.compare(b);
}

let cmp: fun(int, int): int = natural_order;   // inferred as natural_order<int>
```

Because inference needs a target, a bare `let f = natural_order;` is an error — supply a function type via annotation or a matching parameter.

### Type checking inside generic bodies

Use `is` to branch on the concrete type. The compiler eliminates the dead branches:

```dream
fun describe<T>(v: T): void {
    if (v is int) {
        print("it's an int: ");
        println(v);
    } else if (v is string) {
        print("it's a string: ");
        println(v);
    }
}
```

### How it works

Every unique combination of type arguments creates a new instantiation. `Box<int>` and `Box<string>` are entirely separate types in the output — no boxing, no virtual dispatch, no overhead versus hand-written type-specific code.
