# Variables

Variables hold values. In Dream you declare them with `let` (mutable) or `const` (immutable), and the compiler usually figures out the type for you.

## Declaring a variable

Use `let`. The type is inferred from the value on the right:

```dream
let x = 42;          // int
let name = "Alice";  // string
let ratio = 3.14;    // float
let done = false;    // bool
```

Write the type explicitly when the value alone is ambiguous, or when you want a different type than inference would pick:

```dream
let score: double = 99.5d;
let items: int[] = [1, 2, 3];
```

## Reassigning

`let` variables are mutable — assign a new value with `=`:

```dream
let count = 0;
count = count + 1;
```

Compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`) and increment/decrement (`++`, `--`) also work. See [Operators](operators.md).

## Constants

`const` declares a binding you cannot reassign. Trying to is a compile error:

```dream
const pi: int = 3;
// pi = 4;   // error: cannot assign to 'pi' because it is a const binding
```

## Scope

A variable lives until the end of the block it was declared in. When a reference-typed value (string, array, class) leaves scope, its reference count drops automatically — see [Memory Management](memory.md).

```dream
fun main(): void {
    let a = 10;
    {
        let b = 20;    // only alive inside these braces
        println(a + b);
    }
    // b is gone here; a is still fine
}
```

## Type inference rules

Inference reads the initializer. A few defaults to keep in mind:

- Whole-number literals are `int`.
- A literal with a `.` and no suffix is `float`; `3.14f` is also `float`.
- A `d`/`D` suffix makes a `double` (`3.14d`).
- String literals are `string`.

If inference gives you the wrong type, add an annotation or a suffix:

```dream
let pi: double = 3.14159;   // annotation
let pi2 = 3.14159d;         // suffix
```

## Top-level variables

`let` and `const` can also live at the top level of a file, outside any function or class. These become **module globals**: their initializers run once, in declaration order, when the module loads, and a later global may read an earlier one.

```dream
let counter: int = 10;
const FACTOR: int = 3;
let derived: int = counter * FACTOR;   // may reference earlier globals

fun main(): void {
    counter = counter + 5;   // top-level `let` is still mutable
    System.println(derived);
}
```

### Visibility

Top-level variables are **file-private by default**: readable anywhere in their own `.dream` file, but not visible to files that `import` it and not exported. Two modifiers adjust this:

- `public` — importable from other files and exported to the WebAssembly host.
- `static` — kept file-local (the default for a non-public variable, made explicit).

They are mutually exclusive on one declaration:

```dream
public let version: int = 1;   // exported to the host
static let cache: int = 0;     // file-local

// public static let x = 1;    // error: cannot be both 'public' and 'static'
```
