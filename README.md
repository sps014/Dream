# Dream

A fast, statically typed language that compiles straight to WebAssembly. Clean C-like syntax, automatic memory management via ARC (no GC pauses, no manual frees), zero-cost generics, and a batteries-included standard library — all in a compiler written in Rust.

**[Read the docs →](https://sps014.github.io/Dream/)** &nbsp;·&nbsp; [Getting Started](https://sps014.github.io/Dream/getting-started/) &nbsp;·&nbsp; [Language Reference](https://sps014.github.io/Dream/language/variables/) &nbsp;·&nbsp; [Compiler Internals](https://sps014.github.io/Dream/compiler/)

## A taste

```dream
fun greet(name: string): string {
    return "Hello, " + name;
}

// Discriminated unions + pattern matching
enum Shape {
    Circle(radius: float),
    Rect(width: float, height: float),
}

fun area(s: Shape): float {
    return switch (s) {
        Circle(r)  => 3.14 * r * r,
        Rect(w, h) => w * h,
    };
}

fun main() {
    println(greet("world"));

    let shapes = List<Shape>();
    shapes.push(Shape.Circle(2.0));
    shapes.push(Shape.Rect(3.0, 4.0));

    for (let s in shapes) {
        println(area(s));
    }
}
```

## Features

- **Statically typed** — errors caught at compile time, with type inference so you rarely annotate.
- **WebAssembly native** — compiles to `.wat`/`.wasm` that runs in browsers, Node.js, and standalone WASM runtimes.
- **Automatic memory (ARC)** — deterministic reference counting frees values the instant they're unused. No GC, no `free`.
- **Zero-cost generics** — monomorphized to concrete types, no runtime overhead.
- **Rich type system** — classes, value structs, interfaces, enums, discriminated unions, `Option`/`Result`.
- **`async`/`await`** — a cooperative scheduler that runs entirely inside the WASM module, plus `WebWorker` for real multi-core parallelism.
- **Seamless JS interop** — call JavaScript with the `js` type and `extern`, pass callbacks in both directions.
- **Batteries included** — `List`, `Map`, `Set`, strings, JSON (with `@json` auto-derive), files, HTTP, regex, and dates.

## Build

```bash
git clone https://github.com/sps014/Dream
cd Dream
cargo build --release
```

The compiler binary lands at `target/release/dream`.

## Run a program

```bash
cargo run -- run path/to/your/file.dream   # compile and execute
cargo run -- path/to/your/file.dream       # compile to .wat only
```

## Test

```bash
cargo test
```

## License

MIT
