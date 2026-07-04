# Functions

## Defining a function

```dream
fun add(a: int, b: int): int {
    return a + b;
}
```

- `fun` keyword, then the name.
- Parameters are `name: type` pairs separated by commas.
- `: ReturnType` after the parameter list.

The return type is optional for functions that return nothing. These two are equivalent:

```dream
fun greet() {
    println("hi");
}

fun greet(): void {
    println("hi");
}
```

## Calling a function

```dream
let result = add(3, 4);
```

## Default parameter values

A parameter may declare a default value with `= <literal>`. When a caller omits that argument, the default is substituted:

```dream
fun greet(name: string, times: int = 1): void {
    let i = 0;
    while (i < times) {
        println("hi " + name);
        i = i + 1;
    }
}

fun main() {
    greet("Ada");      // times defaults to 1
    greet("Ada", 3);   // times = 3
}
```

Rules:

- A default must be a **constant literal** — a number (optionally negative, e.g. `-1`), `true`/`false`, a string, a char, or `null`. Arbitrary expressions are not allowed.
- Defaults must be **trailing**: once a parameter has a default, every parameter after it must also have one. A required parameter cannot follow a defaulted one.
- A call must still supply all leading required arguments; supplying more than the total parameter count is an error.
- Default parameters work on free functions, methods, and constructors, and may be combined with [overloading](invariants.md#overloading): an exact-arity match wins over an overload that fills in a default, and a genuinely ambiguous call is reported.

Defaults are also honored by constructors and methods:

```dream
class Greeter {
    public factor: int;
    constructor(factor: int = 3) { this.factor = factor; }
    public fun scale(n: int, by: int = 2): int { return n * by * this.factor; }
}

fun main() {
    let g = Greeter();        // factor = 3
    println(g.scale(4));      // 4 * 2 * 3 = 24
    println(g.scale(4, 5));   // 4 * 5 * 3 = 60
}
```

## Returning a value

Use `return`:

```dream
fun clamp(value: int, lo: int, hi: int): int {
    if (value < lo) { return lo; }
    if (value > hi) { return hi; }
    return value;
}
```

The compiler checks that all code paths return a value when the return type is not `void`.

In a `void` function, a bare `return;` exits early with no value:

```dream
fun log_positive(n: int): void {
    if (n < 0) { return; }   // early exit, nothing returned
    println(n);
}
```

## Recursion

Functions can call themselves:

```dream
fun fib(n: int): int {
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}
```

## Generic functions

Add `<TypeParam>` after the function name to make it generic. The type parameter stands in for any concrete type:

```dream
fun identity<T>(value: T): T {
    return value;
}

fun main() {
    println(identity<int>(42));
    println(identity<string>("hello"));
}
```

The compiler generates a separate copy of the function body for each distinct type you use it with. There is no runtime overhead from generics.

Multiple type parameters are allowed:

```dream
fun pair_first<A, B>(a: A, b: B): A {
    return a;
}
```

## First-class functions

A function name used as a value refers to the function itself, and the function type is written `fun(ParamTypes): ReturnType`. Functions can be stored in variables and passed as parameters, then invoked like any other call:

```dream
fun twice(x: int): int {
    return x * 2;
}

fun apply(f: fun(int): int, value: int): int {
    return f(value);
}

fun main() {
    let g: fun(int): int = twice;
    println(g(5));            // 10
    println(apply(twice, 8)); // 16
}
```

Closures (capturing surrounding variables) are not yet supported.

## Public functions

Functions are **file-private by default** — callable anywhere within their own `.dream` file, but neither visible to other files that `import` it nor exposed to the host. Mark a function `public` to make it importable from other files and to export it to the WebAssembly host environment (see [Imports > Visibility](imports.md#visibility)):

```dream
public fun compute(n: int): int {
    return n * n;
}
```

A `public` function cannot expose a class that is not itself `public`.

## Entry point

The runtime calls `main` to start the program. Every runnable Dream program needs one. The return type can be omitted:

```dream
fun main() {
    println("hello");
}
```

## Imports

Split a program across files with `import`. See [Imports](imports.md).
