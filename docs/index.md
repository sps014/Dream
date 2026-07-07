# Dream

Dream is a fast, statically typed language that compiles straight to WebAssembly. It has a clean, C-like syntax and manages memory for you with automatic reference counting (ARC) — no garbage collector pauses, no manual frees.

```dream
fun greet(name: string): string {
    return "Hello, " + name;
}

fun main() {
    println(greet("world"));
}
```

New here? Start with [Getting Started](getting-started.md), then work through the Basics. Already comfortable? Jump to the [language reference](language/variables.md) or the [standard library](stdlib/builtins.md).

## Why Dream

<div class="grid cards" markdown>

-   :material-shield-check: **Statically typed**

    ---

    Mistakes are caught at compile time, not in production.

-   :material-web: **WebAssembly native**

    ---

    Compiles to a `.wat`/`.wasm` module that runs in browsers, Node.js, and standalone WASM runtimes.

-   :material-memory: **Automatic memory**

    ---

    Deterministic ARC frees values the moment they are no longer used. No GC, no manual `free`.

-   :material-shape: **Zero-cost generics**

    ---

    Reusable code that monomorphizes to concrete types — no runtime overhead.

-   :material-library: **Batteries included**

    ---

    `List`, `Map`, `Set`, strings, JSON, files, HTTP, and dates ship in the standard library.

</div>

## Start here

<div class="grid cards" markdown>

-   :material-rocket-launch: **Getting Started**

    ---

    Install the compiler, write your first program, and run it.

    [:octicons-arrow-right-24: Install & run](getting-started.md)

-   :material-book-open-page-variant: **Language Basics**

    ---

    Variables, operators, control flow, and functions.

    [:octicons-arrow-right-24: Learn the syntax](language/variables.md)

-   :material-cube-outline: **Types & Data**

    ---

    Primitives, arrays, enums, unions, classes, and structs.

    [:octicons-arrow-right-24: Model your data](language/types.md)

-   :material-cog: **Compiler Internals**

    ---

    How the compiler turns `.dream` source into WebAssembly, for contributors.

    [:octicons-arrow-right-24: Read the handbook](compiler/README.md)

</div>

## Standard library

<div class="grid cards" markdown>

-   :material-code-braces: **Built-ins**

    ---

    Core utilities: `print`, `println`, `to_string`, `hash_code`, `Array<T>`, `Buffer`, `Math`.

    [:octicons-arrow-right-24: Built-ins](stdlib/builtins.md)

-   :material-format-text: **Strings**

    ---

    Slicing, searching, casing, splitting, and formatting.

    [:octicons-arrow-right-24: Strings](stdlib/string.md)

-   :material-alert-circle-outline: **Option & Result**

    ---

    Handle missing values and errors without null.

    [:octicons-arrow-right-24: Option & Result](stdlib/option-result.md)

-   :material-layers: **Collections**

    ---

    `List<T>`, `Map<K, V>`, and `Set<T>`.

    [:octicons-arrow-right-24: Collections](stdlib/collections.md)

-   :material-code-json: **JSON**

    ---

    Parse, build, and stringify JSON, plus `@json` auto-derive.

    [:octicons-arrow-right-24: JSON](stdlib/json.md)

-   :material-file-document: **File I/O**

    ---

    Read, write, and stream files with `File` and `FileStream`.

    [:octicons-arrow-right-24: File I/O](stdlib/file.md)

-   :material-swap-horizontal: **HTTP**

    ---

    Cross-runtime requests over `async`/`await` with `HttpClient`.

    [:octicons-arrow-right-24: HTTP](stdlib/http.md)

-   :material-calendar-clock: **DateTime**

    ---

    Dates, times, arithmetic, and comparisons.

    [:octicons-arrow-right-24: DateTime](stdlib/datetime.md)

</div>

## Interop

Dream runs inside the browser, Node.js, and native WASM runtimes. The `extern` keyword and the `js` type bridge to the JavaScript host with no boilerplate.

<div class="grid cards" markdown>

-   :material-javascript: **JS interop**

    ---

    Call JavaScript from Dream and expose Dream to JavaScript.

    [:octicons-arrow-right-24: Interop overview](language/interop.md)

-   :material-link: **The `js` type**

    ---

    Read, call, and mutate live JavaScript values with native syntax.

    [:octicons-arrow-right-24: The js type](language/references.md)

-   :material-phone-in-talk: **Callbacks**

    ---

    Pass functions in both directions across the boundary.

    [:octicons-arrow-right-24: Callbacks](language/callbacks.md)

</div>
