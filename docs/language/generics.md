# Generics

Generics let you write code that works for any type without duplicating it. Dream resolves generics at compile time — the compiler produces a separate, fully optimized copy of your code for each concrete type you use.

## Generic functions

Add `<T>` after the function name:

```dream
fun first<T>(arr: T[]): T {
    return arr[0];
}

fun main(): void {
    let nums = [10, 20, 30];
    let words = ["a", "b", "c"];
    println(first<int>(nums));     // 10
    println(first<string>(words)); // a
}
```

The type argument can often be inferred from the call site, though explicit `<Type>` is always accepted.

Multiple type parameters:

```dream
fun swap<A, B>(a: A, b: B): A {
    return a;
}
```

### As a first-class value

A generic function can be used as a [first-class function value](functions.md). Its type arguments are
inferred from the expected function type at the use site, then that instance is monomorphized like any
other. This is how `List<T>.sort()` reuses the single merge sort in `sort_by`:

```dream
fun natural_order<T : Comparable<T>>(a: T, b: T): int {
    return a.compare(b);
}

fun main(): void {
    let cmp: fun(int, int): int = natural_order;   // inferred as natural_order<int>
    // cmp can now be passed anywhere a `fun(int, int): int` is expected.
}
```

Because inference needs a target type, a generic function used as a value requires a known function
type in context (an annotation or a matching parameter); a bare `let f = natural_order;` is an error.

## Generic classes

Classes can be generic too:

```dream
class Pair<A, B> {
    first: A;
    second: B;
}

fun main(): void {
    let p = Pair<int, string>(1, "one");
    println(p.first);
    println(p.second);
}
```

Type arguments can themselves be generic (or arrays), so generics nest freely:

```dream
let nested = Pair<Box<int>, int>(Box<int>(7), 5);
println(nested.first.v);   // 7

let pts: Pair<int, int>[] = [Pair<int, int>(1, 2)];
println(pts[0].second);    // 2
```

## Generic methods

Methods on generic classes automatically have access to the class's type parameters:

```dream
class Box<T> {
    value: T;

    fun get(): T {
        return this.value;
    }

    fun set(v: T): void {
        this.value = v;
    }
}

fun main(): void {
    let b = Box<int>(42);
    b.set(100);
    println(b.get());   // 100
}
```

## Generic constraints

A type parameter can be **constrained** to one or more interfaces with `T : Iface`. Inside the body,
a constrained parameter exposes the interface's methods, so you can call them on values of that type.
Constraints apply to generic functions, classes, structs, interfaces, and `extend` blocks.

```dream
fun max_of<T : Comparable<T>>(a: T, b: T): T {
    if (a.compare(b) > 0) {   // `compare` is available because T : Comparable<T>
        return a;
    }
    return b;
}
```

Combine several bounds with `+`:

```dream
struct Sorted<T : Comparable<T> + Equatable<T>> { /* ... */ }
```

At every instantiation the compiler checks that the concrete type actually satisfies the constraint,
reporting an error otherwise (e.g. `List<int>.sort()` fails unless `int` implements `Comparable<int>`).
Because generics are monomorphized, a constrained call binds to the concrete type's method — ordinary
**static dispatch, with no boxing**, even for [value structs](classes-structs.md). This is what lets a
value struct satisfy `Comparable`/`Equatable` and be sorted or compared without ever allocating.

## Type checking inside generic bodies

Use `is` to branch on the concrete type at compile time. The compiler eliminates dead branches entirely:

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

## How it works

Every unique combination of type arguments creates a new instantiation. `Box<int>` and `Box<string>` are entirely separate types in the compiled output. There is no boxing, no virtual dispatch, and no runtime overhead compared to writing the type-specific code by hand.
