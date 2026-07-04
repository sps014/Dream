# Variables

## Declaring a variable

Use `let` to declare a local variable. The type is inferred from the right-hand side:

```dream
let x = 42;          // int
let name = "Alice";  // string
let ratio = 3.14;    // float
let done = false;    // bool
```

You can also write the type explicitly. This is required when the initializer alone is ambiguous:

```dream
let score: double = 99.5d;
let items: int[] = [1, 2, 3];
```

## Constants

Use `const` instead of `let` to declare an immutable binding. Reassigning a `const` is a compile error:

```dream
const pi: int = 3;
// pi = 4;   // error: cannot assign to 'pi' because it is a const binding
```

## Assignment

Variables declared with `let` are mutable. Assign a new value with `=`:

```dream
let count = 0;
count = count + 1;
```

Compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`) and increment/decrement (`++`, `--`) are also supported (see [operators](operators.md)).

## Top-level variables

`let` and `const` can also be declared at the top level of a file, outside any function or class. These become module globals: their initializers run once, in declaration order, when the module is instantiated, and a later global may reference an earlier one.

```dream
let counter: int = 10;
const FACTOR: int = 3;
let derived: int = counter * FACTOR;   // may reference earlier globals

fun main(): void {
    counter = counter + 5;             // top-level `let` is mutable
    System.println(derived);
}
```

Top-level variables are **file-private by default** — readable anywhere within their own `.dream` file but not visible to other files that `import` it, and not exported. Two modifiers adjust this:

- `public` — make the variable importable from other files and export it to the WebAssembly host.
- `static` — keep the variable file-local (its current meaning; a non-public variable is already file-private).

`public` and `static` are mutually exclusive on the same declaration:

```dream
public let version: int = 1;   // exported to the host
static let cache: int = 0;     // file-local

// public static let x = 1;    // error: cannot be both 'public' and 'static'
```

A top-level `const` is immutable just like a local one; reassigning it is a compile error.

## Scope

Variables live until the end of the block they are declared in. When a reference-typed variable (string, array, class) goes out of scope, its reference count is decremented automatically.

```dream
fun main(): void {
    let a = 10;
    {
        let b = 20;        // b is only alive here
        println(a + b);
    }
    // b is gone here; a is still fine
}
```

## Type inference rules

The compiler infers the type from the initializer expression. A few things to watch out for:

- Number literals without a suffix are `int`.
- Literals ending in `f`/`F` are `float` (`3.14f`).
- Literals ending in `d`/`D` are `double` (`3.14d`).
- Literals with a `.` but no suffix are also `float`.
- String literals are `string`.

If inference gives you the wrong type, add an explicit annotation or a suffix:

```dream
let pi: double = 3.14159;   // explicit annotation
let pi2 = 3.14159d;         // suffix
```
