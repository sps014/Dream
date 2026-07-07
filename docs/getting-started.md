# Getting Started

This page gets you from nothing to a running Dream program in a few minutes.

## Prerequisites

You only need [Rust](https://rustup.rs) installed. The Dream compiler builds with `cargo`.

## Install

```bash
git clone https://github.com/sps014/MinLang
cd Dream
cargo build --release
```

The compiler binary lands at `target/release/dream`. You can call that path directly, or use `cargo run --` while developing (the examples below use `cargo run`).

## Your first program

Create `hello.dream`:

```dream
fun main() {
    println("Hello, world!");
}
```

Run it:

```bash
cargo run -- run hello.dream
```

```
Hello, world!
```

The `run` subcommand compiles and immediately executes your file with Wasmtime. To inspect the generated WebAssembly instead, drop `run`:

```bash
cargo run -- hello.dream
```

That writes `hello.wat` (WebAssembly text) next to your source.

## A bigger example

```dream
fun factorial(n: int): int {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

fun main() {
    let i = 1;
    while (i <= 10) {
        println(factorial(i));
        i = i + 1;
    }
}
```

A few things to notice:

- `fun` declares a function; its return type follows the `:`.
- The return type is optional when a function returns nothing, as in `fun main()`.
- `let` declares a local; its type is inferred from the initializer.
- `println` works on any type — `int`, `float`, `string`, `bool`, `char`, and your own classes.
- Conditions are parenthesized: `if (n <= 1)`.

## Where to go next

- [Variables](language/variables.md) — declaration, inference, and scope.
- [Control Flow](language/control-flow.md) — `if`, `while`, `for`, and `switch`.
- [Types & Data](language/types.md) — the full type landscape.
- [Classes & Structs](language/classes-structs.md) — define your own types with methods.
- [Collections](stdlib/collections.md) — `List<T>`, `Map<K, V>`, and `Set<T>`.
