# Getting Started

This page gets you from nothing to a running Dream program in a few minutes.

## Prerequisites

You only need [Rust](https://rustup.rs) installed. The Dream compiler builds with `cargo`.

## Install

```bash
git clone https://github.com/sps014/Dream
cd Dream
cargo build --release
```

The compiler binary lands at `target/release/dream`. You can call that path directly, or use `cargo run --` while developing (the examples below use `cargo run`).

## Your first program

Create `hello.dream`:

```dream
import system;

fun main() {
    System.println("Hello, world!");
}
```

Run it:

```bash
cargo run -- run hello.dream
```

```
Hello, world!
```

`import system;` loads the console / process package so `System.println` is available. Other stdlib surfaces use their own packages (`system.collections`, `system.net`, …) — see [Imports](language/imports.md#standard-library-packages). The editor can insert these for you via an auto-import quick fix.

The `run` subcommand compiles to WebAssembly and executes your program. To compile without running:

```bash
cargo run -- hello.dream
```

That writes a `.wasm` (and related artifacts) next to your source.

## A bigger example

```dream
import system;

fun factorial(n: int): int {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

fun main() {
    let i = 1;
    while (i <= 10) {
        System.println(factorial(i));
        i = i + 1;
    }
}
```

A few things to notice:

- `fun` declares a function; its return type follows the `:`.
- The return type is optional when a function returns nothing, as in `fun main()`.
- `let` declares a local; its type is inferred from the initializer.
- `System.println` works on any type — `int`, `float`, `string`, `bool`, `char`, and your own classes.
- Conditions are parenthesized: `if (n <= 1)`.

## Where to go next

- [Variables](language/variables.md) — declaration, inference, and scope.
- [Control Flow](language/control-flow.md) — `if`, `while`, `for`, and `switch`.
- [Types & Data](language/types.md) — the full type landscape.
- [Classes & Structs](language/classes-structs.md) — define your own types with methods.
- [Collections](stdlib/collections.md) — `List<T>`, `Map<K, V>`, and `Set<T>` (`import system.collections;`).
- [Imports](language/imports.md) — file imports and `system.*` packages.
