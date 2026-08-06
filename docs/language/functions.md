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

- A default must be a **constant literal**: a number (may be negative), `true`/`false`, a string, or a char. No arbitrary expressions.
- Defaults must be **trailing** — once one parameter has a default, all after it must too.
- Callers must still pass every leading required argument; passing more than the total is an error.

Defaults also apply to constructors and methods:

```dream
class Greeter {
    public factor: int;
    public constructor(factor: int = 3) { this.factor = factor; }
    public fun scale(n: int, by: int = 2): int { return n * by * this.factor; }
}

let g = Greeter();        // factor = 3
println(g.scale(4));      // 4 * 2 * 3 = 24
println(g.scale(4, 5));   // 4 * 5 * 3 = 60
```

## Named arguments

A call may label an argument with its parameter's name (`name: value`) instead of relying on
position:

```dream
fun greet(name: string, greeting: string = "Hello", punctuation: string = "!"): string {
    return greeting + ", " + name + punctuation;
}

greet(name: "Ada");                          // "Hello, Ada!"
greet(name: "Lin", greeting: "Hi");          // "Hi, Lin!"
greet(greeting: "Hi", name: "Grace");        // "Hi, Grace!" — order doesn't matter
greet("Bob", punctuation: "?");              // "Hello, Bob?" — positional + named mixed
greet(name: "Amy", punctuation: ".");        // "Hello, Amy." — skips `greeting`, its default fills in
```

Rules:

- **Every positional argument must come before every named one** — `greet(name: "Ada", "Hi")` is
  an error.
- A name must match a declared parameter of the callee; each name may be used **at most once** per
  call.
- Naming an argument doesn't require naming every argument after it — any parameter left unfilled
  once positional and named arguments are assigned falls back to its default value (an error if it
  has none).
- Named arguments work for **free functions, constructors, and instance methods**, including
  overloaded callees: each overload's parameter names are tried, then argument types pick among
  the survivors (an ambiguous name layout across overloads is an error). Use positional arguments
  when you prefer not to involve names.

Named arguments and defaults compose: they're the mechanism that lets a call skip a *middle*
optional parameter while still supplying a later one, which trailing-only default omission alone
cannot express:

```dream
class Rect {
    public width: int;
    public height: int;
    public constructor(width: int, height: int = 1) { this.width = width; this.height = height; }
    public fun area(scale: int = 1, offset: int = 0): int {
        return (this.width * this.height * scale) + offset;
    }
}

let r = Rect(width: 3, height: 4);
println(r.area(offset: 5));   // scale keeps its default (1): 3*4*1 + 5 = 17
```

## Variadic parameters

A function's **last** parameter can be marked `...name: T[]` to accept zero or more trailing `T`
arguments, collected into an array bound to `name` inside the body:

```dream
fun sum(...nums: int[]): int {
    let total = 0;
    for (let n in nums) {
        total = total + n;
    }
    return total;
}

sum();          // total = 0  (nums is an empty array)
sum(1);         // total = 1
sum(1, 2, 3);   // total = 6
```

A variadic parameter can follow ordinary (including defaulted) parameters, as long as it is last:

```dream
fun sum_with_base(base: int, ...nums: int[]): int {
    let total = base;
    for (let n in nums) {
        total = total + n;
    }
    return total;
}

sum_with_base(10);           // 10
sum_with_base(10, 1, 2, 3);  // 16
```

Rules:

- Only the **last** parameter may be variadic; a required or defaulted parameter cannot follow it.
- Its declared type must be an array type (`T[]`); the caller passes bare `T` values, not an
  already-built array — there is exactly one calling convention, not two.
- Variadic parameters work for **free functions, constructors, and instance methods**, including
  overloaded callees: trailing arguments are matched against the variadic element type during
  overload resolution, then packed into the `T[]` parameter.
- Named arguments and variadic parameters compose in one call: name the **fixed** parameters, then
  supply any trailing variadic elements positionally — e.g. `sum_with_base(base: 10, 1, 2, 3)`.
  The variadic parameter itself cannot be passed by name (there is one calling convention: bare
  `T` values, not a pre-built array).

## Pass by reference (`ref`)

A parameter marked `ref name: T` shares the caller's storage instead of receiving a copy: writes
to it inside the callee are visible to the caller once the call returns (and, for anything the
caller can still observe concurrently — a captured local, see below — immediately). This mirrors
C#'s `ref`: the modifier is required on **both** the declaration and every call site, so nothing
about a call's aliasing behavior is implicit.

```dream
fun swap(ref a: int, ref b: int): void {
    let tmp: int = a;
    a = b;
    b = tmp;
}

let p: int = 1;
let q: int = 2;
swap(ref p, ref q);
println(p);   // 2
println(q);   // 1
```

`ref` works the same way on instance and static methods (the implicit `this` is unaffected — `ref`
only ever applies to the explicit parameter list):

```dream
struct Doubler {
    public fun apply(ref x: int): void {
        x = x * 2;
    }
}

let d: Doubler = Doubler();
let n: int = 5;
d.apply(ref n);
println(n);   // 10
```

Rules:

- `ref` must appear at the call site (`f(ref x)`), not just the declaration — omitting it, or
  adding it where the parameter isn't `ref`, is a compile-time error.
- A `ref` argument's target may be a local variable, a parameter, a struct/class field, or an
  array element (`f(ref x)`, `f(ref obj.field)`, `f(ref arr[i])`).
- `ref` cannot combine with a default value or a variadic (`...`) parameter on the same parameter.
- A lambda may declare `ref` parameters. Annotate the function type as `fun(ref T): R` (the
  `ref` markers are part of the type) and call with `f(ref x)`:
  ```dream
  let inc: fun(ref int): void = (ref n: int) => { n = n + 1; };
  let a: int = 5;
  inc(ref a);
  println(a); // 6
  ```
- A lambda **cannot capture** an enclosing function's `ref` parameter, even though it can capture
  an ordinary `let`/parameter (see [Capturing closures](#capturing-closures)). A `ref`
  parameter's storage is only guaranteed to live for the duration of the call it came from; a
  capturing lambda could outlive that call (e.g. by being returned), which would leave it holding
  a dangling reference. This is a compile-time error, mirroring C#'s rule against capturing
  `ref`/`out` parameters.

### `ref` and closures

A `ref` argument aliases the caller's storage. If that local is also captured by a closure, both
see the same storage — mutations through the `ref` and through the closure are visible to both:

```dream
fun increment(ref x: int): void {
    x = x + 1;
}

fun main(): void {
    let counter: int = 0;
    let inc: fun(): int = () => {
        increment(ref counter);   // ref-passes the closure's own captured storage
        return counter;
    };
    println(inc());     // 1
    println(inc());     // 2
    println(counter);   // 2 — visible to the enclosing scope too

    increment(ref counter);   // and the enclosing scope's writes are visible to the closure
    println(inc());           // 4
}
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

A [generic function used as a first-class value](generics.md#generic-functions-as-first-class-values) needs a `fun(...)`-typed context so its type arguments can be inferred — e.g. `let cmp: fun(int, int): int = natural_order;` — a bare `let f = natural_order;` is an error.

#### Arrow-lambda literals

An anonymous function can be written inline with arrow syntax, `(params) => expr` or `(params) => { statements }`. A parameter's `: Type` annotation is optional when the lambda is used in a `fun(...)`-typed context (a `let` annotation, or a parameter/argument whose declared type is `fun(...)`): omitted parameter types and the return type are taken from that context. When every parameter is explicitly annotated, the return type can instead be inferred from the body — so `let f = (x: int) => x * 2;` works without a surrounding `fun(...)` annotation. A lambda with any untyped parameter and no `fun(...)` context is rejected.

```dream
let add: fun(int, int): int = (x, y) => x + y;   // x, y inferred as int from the `let` annotation
println(add(2, 3));   // 5

let square: fun(int): int = (x) => {
    let r = x * x;
    return r;
};
println(square(5));   // 25

let nums: List<int> = List<int>();
nums.push(3);
nums.push(1);
nums.push(2);
nums.sort_by((a, b) => a - b);   // `a`/`b` inferred as `int` from `sort_by`'s `cmp: fun(int, int): int`
```

A lambda written with an untyped parameter and no surrounding `fun(...)` context cannot have its type inferred and is rejected with a diagnostic asking for one. A lambda may declare its own type parameters (`<T>(x: T) => x`), which monomorphize like a generic function — from a `fun(...)` context or by binding a polymorphic item and instantiating at each use.

#### Async lambdas

Prefix an arrow lambda with `async` to allow `await` in its body. An async lambda is a first-class value of type `fun(...): Future<T>` — calling it eagerly starts the task and returns a `Future` handle, same as calling a named `async fun`:

```dream
async fun main(): void {
    let twice: fun(int): Future<int> = async (x) => {
        await Time.sleep(1);
        return x * 2;
    };
    let n = await twice(21);   // twice(21) : Future<int>
    println(n);                // 42
}
```

The expected context must be `fun(...): Future<T>` (not `fun(...): T`). A sync lambda against a `Future`-returning `fun` type, or an async lambda against a non-`Future` return, is a compile-time error. Named `async fun` values work the same way — `let f: fun(int): Future<int> = delayed_triple;` boxes the function as a `Future`-returning `fun` value.

#### Capturing closures

A lambda's body may also reference variables from an enclosing function; this is a *capture*. A captured name is captured **by reference**, not by value: the closure and the enclosing function share the same storage, so a write from either side is visible to the other, and the closure keeps working after the enclosing function has returned. A lambda may capture more than one name, and capture is transitive: a lambda nested inside another lambda may reach past its immediate parent to a grandparent's (or higher) local — each level forwards what the level below it needs, one hop at a time.

```dream
fun make_adder(n: int): fun(int): int {
    return (x) => x + n;   // `x` inferred from the return type; captures the parameter `n`
}

let add5: fun(int): int = make_adder(5);
println(add5(10));   // 15
println(add5(20));   // 25

fun make_counter(): fun(): int {
    let count: int = 0;
    return () => {
        count = count + 1;   // mutates the enclosing `let` — visible next call, and to `count`
        return count;        // itself if it's still in scope when this returns
    };
}

let inc: fun(): int = make_counter();
println(inc());   // 1
println(inc());   // 2
```

Each call to a function that returns a capturing lambda creates its own independent storage — two counters from separate `make_counter()` calls do not interfere with each other.

Capturing closures are ordinary `fun(...)` values inside Dream, but they **cannot** be passed to JavaScript APIs — the JS bridges drop the closure environment. See [Callbacks](callbacks.md).

See [Pass by reference (`ref`)](#pass-by-reference-ref) for how a captured variable composes with a `ref` parameter on another function (they share the same underlying storage), and why a lambda cannot capture an enclosing `ref` parameter itself.

Capturing more than one variable, and reaching past an immediate parent lambda to a grandparent's local, both work the same way:

```dream
let a: int = 1;
let b: int = 2;
let f: fun(): int = () => a + b;   // captures both `a` and `b`
println(f());   // 3

fun make(a: int, b: int): fun(): fun(): int {
    let c: int = 100;
    // The outer lambda doesn't reference `a`/`b`/`c` itself, but forwards all three to the inner
    // one, which does — a multi-level, multi-capture chain.
    return () => {
        return () => {
            c = c + 1;
            return a + b + c;
        };
    };
}

let l1: fun(): fun(): int = make(1, 2);
let l2: fun(): int = l1();
println(l2());   // 104
println(l2());   // 105
```

### Overloading

Multiple functions can share a name if their parameters differ; see [Language Invariants](invariants.md#overloading). An exact-arity match wins over one that fills in a default, and truly ambiguous calls are reported.
