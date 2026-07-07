# Functions

Functions are declared with `fun`. They take typed parameters, optionally return a value, and can be generic or passed around as values.

## Defining and calling

```dream
fun add(a: int, b: int): int {
    return a + b;
}

let result = add(3, 4);
```

- `fun`, then the name.
- Parameters are `name: type`, comma-separated.
- `: ReturnType` follows the parameter list.

The return type is optional for functions that return nothing, so these are equivalent:

```dream
fun greet() { println("hi"); }
fun greet(): void { println("hi"); }
```

## Returning a value

Use `return`. The compiler checks that every path returns when the return type is not `void`:

```dream
fun clamp(value: int, lo: int, hi: int): int {
    if (value < lo) { return lo; }
    if (value > hi) { return hi; }
    return value;
}
```

In a `void` function a bare `return;` exits early:

```dream
fun log_positive(n: int): void {
    if (n < 0) { return; }
    println(n);
}
```

Functions can call themselves — recursion works as expected:

```dream
fun fib(n: int): int {
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}
```

## Default parameter values

A parameter can supply a default with `= <literal>`; callers may then omit it:

```dream
fun greet(name: string, times: int = 1): void {
    let i = 0;
    while (i < times) {
        println("hi " + name);
        i = i + 1;
    }
}

greet("Ada");      // times = 1
greet("Ada", 3);   // times = 3
```

Rules:

- A default must be a **constant literal**: a number (may be negative), `true`/`false`, a string, a char, or `null`. No arbitrary expressions.
- Defaults must be **trailing** — once one parameter has a default, all after it must too.
- Callers must still pass every leading required argument; passing more than the total is an error.

Defaults also apply to constructors and methods:

```dream
class Greeter {
    public factor: int;
    constructor(factor: int = 3) { this.factor = factor; }
    public fun scale(n: int, by: int = 2): int { return n * by * this.factor; }
}

let g = Greeter();        // factor = 3
println(g.scale(4));      // 4 * 2 * 3 = 24
println(g.scale(4, 5));   // 4 * 5 * 3 = 60
```

## Public functions and entry point

Functions are **file-private by default**. Mark one `public` to import it from other files and export it to the WebAssembly host (see [Imports](imports.md#visibility)). A `public` function cannot expose a non-`public` class.

```dream
public fun compute(n: int): int {
    return n * n;
}
```

The runtime starts a program by calling `main`. Every runnable program needs one; its return type can be omitted:

```dream
fun main() {
    println("hello");
}
```

## Advanced

### Generic functions

Add `<TypeParam>` after the name. The compiler emits a separate copy per concrete type used — no runtime cost. See [Generics](generics.md).

```dream
fun identity<T>(value: T): T {
    return value;
}

println(identity<int>(42));
println(identity<string>("hello"));

fun pair_first<A, B>(a: A, b: B): A { return a; }
```

### First-class functions

A function name is a value; its type is written `fun(ParamTypes): ReturnType`. Store functions in variables, pass them, and call them like any other:

```dream
fun twice(x: int): int { return x * 2; }

fun apply(f: fun(int): int, value: int): int {
    return f(value);
}

let g: fun(int): int = twice;
println(g(5));            // 10
println(apply(twice, 8)); // 16
```

Closures (capturing surrounding variables) are not yet supported.

### Overloading

Multiple functions can share a name if their parameters differ; see [Language Invariants](invariants.md#overloading). An exact-arity match wins over one that fills in a default, and truly ambiguous calls are reported.
