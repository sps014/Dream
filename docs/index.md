# Dream

Dream is a fast, statically typed language that compiles directly to WebAssembly. It features a clean, C-like syntax and manages memory automatically via ARC — no garbage collector pauses, no manual frees.

```dream
fun greet(name: string): string {
    return "Hello, " + name;
}

fun main() {
    println(greet("world"));
}
```

## Features

- **Statically Typed** — errors caught at compile time.
- **WebAssembly Native** — compiles to a `.wat` file for any WASM runtime.
- **Automatic Memory** — deterministic, fast reference counting.
- **Generics** — write reusable code without performance loss.
- **Built-in Collections** — `List<T>` and `Map<K, V>` are standard.

## Start here

New to Dream? Follow the [Getting Started](getting-started.md) guide to install the compiler, write your first program, and run it.

If you already know the basics, the [Language Basics](language/variables.md) section covers everything in detail.

## Standard Library

| Page | Description |
|------|-------------|
| [Built-ins](stdlib/builtins.md) | `print`, `println`, `x.to_string()`, `x.hash_code()`, `Array.new`, `Math.*` |
| [string](stdlib/string.md) | String methods: `substring`, `contains`, `trim`, `to_lower`, … |
| [Primitives](language/primitives.md) | `int`, `float`, `bool`, `char`, `byte`, `long`, etc. |
| [Option & Result](stdlib/option-result.md) | Safe handling of missing values and errors |
| [Collections](stdlib/collections.md) | `List<T>` and `Map<K, V>` |
| [JSON](stdlib/json.md) | `JsonValue` model, `JSON.parse`/`stringify`, `@json` auto-derive |
| [File I/O](stdlib/file.md) | `File` and `FileStream`: read/write text and bytes, list, stat, stream |
| [HTTP](stdlib/http.md) | `HttpClient`: cross-runtime requests over `async`/`await` |
| [DateTime](stdlib/datetime.md) | Calendar dates and times: construction, arithmetic, comparison |

## Interop

Dream runs in the browser, Node, and native WASM runtimes. The `extern` keyword bridges to the JavaScript host with no boilerplate. See [JS Interop](language/interop.md), [References](language/references.md), and [Callbacks](language/callbacks.md).