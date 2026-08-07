# Dream

A fast, statically typed language that compiles straight to WebAssembly. Syntax closer to Rust and TypeScript, automatic memory management via ARC (no GC pauses), zero-cost generics, and a batteries-included standard library — compiler written in Rust.

**[Read the docs →](https://sps014.github.io/Dream/)** &nbsp;·&nbsp; [Getting Started](https://sps014.github.io/Dream/getting-started/) &nbsp;·&nbsp; [Language](https://sps014.github.io/Dream/language/variables/) &nbsp;·&nbsp; [JS interop](https://sps014.github.io/Dream/language/interop/) &nbsp;·&nbsp; [Compiler](https://sps014.github.io/Dream/compiler/)

## A taste

```dream
import system;
import system.collections;

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
    System.println(greet("world"));

    let shapes = List<Shape>();
    shapes.push(Shape.Circle(2.0));
    shapes.push(Shape.Rect(3.0, 4.0));

    for (let s in shapes) {
        System.println(area(s));
    }
}
```

Stdlib APIs live under `system.*` packages — `import system;` for console I/O, `import system.collections;` for `List`/`Map`/`Set`, and so on. Bootstrap types like `Option` and `Result` need no import. See [Imports](https://sps014.github.io/Dream/language/imports/).

## Language features

| Area | What you get |
|------|----------------|
| **Types** | Inference, classes, value structs, interfaces, enums, discriminated unions, `Option`/`Result` |
| **Generics** | Zero-cost monomorphization to concrete WASM |
| **Memory** | Deterministic ARC — no GC pauses, no manual `free` |
| **Concurrency** | `async`/`await` with an in-module cooperative scheduler; `WebWorker` for real parallelism |
| **JS interop** | Dynamic `js` type, `extern fun`, callbacks both ways, optional tree-shaken `*.web.runtime.js` / `*.node.runtime.js` |
| **GPU** | `@compute` kernels → WGSL + `system.gpu` (WebGPU) |
| **Metaprogramming** | `@json` and source generators |
| **Stdlib** | Collections, strings/regex, JSON, files, HTTP, logging, crypto, GPU |

Also: WASM-native output (`.wat` / `.wasm` + `.abi.json`), editor support (VS Code / LSP), and a Rust-hosted `dream run` path via wasmtime.

## Build

```bash
git clone https://github.com/sps014/Dream
cd Dream
cargo build --release
```

The compiler binary lands at `target/release/dream`.

## Run a program

```bash
cargo run -- run path/to/your/file.dream   # compile and execute (native host)
cargo run -- path/to/your/file.dream       # compile to .wat / .wasm / .abi.json

# Tree-shaken JS host for browser or Node (optional)
cargo run -- --runtime --web path/to/your/file.dream
cargo run -- --runtime --node path/to/your/file.dream
```

JS interop: [docs](https://sps014.github.io/Dream/language/interop/) · [`docs/language/interop.md`](docs/language/interop.md).

## Test

```bash
cargo test
```

## License

MIT
