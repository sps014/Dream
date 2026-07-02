# Getting Started

## Prerequisites

You need [Rust](https://rustup.rs) installed. That's it.

## Install

```bash
git clone https://github.com/sps014/MinLang
cd Dream
cargo build --release
```

The binary ends up at `target/release/dream`. You can run it directly from that path, or use `cargo run --` as shown below.

## Your First Program

Create a file called `hello.dream`:

```dream
fun main() {
    println("Hello, world!");
}
```

Run it:

```bash
cargo run -- run hello.dream
```

Output:

```
Hello, world!
```

The `run` subcommand compiles your file and executes it immediately using Wasmtime. To inspect the generated WebAssembly instead, drop the `run` subcommand:

```bash
cargo run -- hello.dream
```

This writes a `hello.wat` file next to your source.

## A Slightly Bigger Example

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

Things to notice:

- `fun` declares a function. The return type comes after `:`.
- `let` declares a local variable. The type is inferred.
- `print` and `println` work on any type — int, float, string, bool, char, classes.
- The return type is optional when a function returns nothing (`fun main()`).
- Conditions need parentheses: `if (n <= 1)`.

## Next Steps

- [Variables](language/variables.md) — declaration, inference, and assignment rules.
- [Types & Data](language/primitives.md) — primitives, collections, and unions.
- [Classes & Structs](language/classes-structs.md) — define your own data types with methods.
- [Collections](stdlib/collections.md) — `List<T>` and `Map<K, V>`.