# Dream

Dream is a fast, statically typed language that compiles straight to WebAssembly. It has a clean, C-like syntax and manages memory for you with automatic reference counting (ARC) — no garbage collector pauses, no manual frees.

```dream
import system;

fun greet(name: string): string {
    return "Hello, " + name;
}

fun main() {
    System.println(greet("world"));
}
```

New here? Start with [Getting Started](getting-started.md), then work through the Basics. Already comfortable? Jump to the [language reference](language/variables.md) or the [standard library](stdlib/builtins.md).

Stdlib APIs are organized as `system.*` packages (`import system.collections;`, `import system.net;`, …). See [Imports](language/imports.md#standard-library-packages).

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

    `List`, `Map`, `Set`, strings, JSON, files, HTTP, and dates via `import system…;`.

</div>

## Start here

<div class="grid cards" markdown>

-   :material-rocket-launch: **Getting Started**

    ---

    Install the compiler, write your first program, and run it.

    [:octicons-arrow-right-24: Install & run](getting-started.md)

-   :material-book-open-page-variant: **Language Basics**

    ---

    Variables, operators, control flow, functions, comments, panics.

    [:octicons-arrow-right-24: Learn the syntax](language/variables.md)

-   :material-cube-outline: **Types & Data**

    ---

    Primitives, arrays, enums, unions, classes, structs, and `object`.

    [:octicons-arrow-right-24: Model your data](language/types.md)

-   :material-auto-fix: **Source Generators**

    ---

    Compile-time derives (`@json`) and custom syntax DSLs (`html { … }`).

    [:octicons-arrow-right-24: Generators guide](language/generators.md)

-   :material-package-variant: **Packages**

    ---

    Publish and consume libraries with `dreamer`.

    [:octicons-arrow-right-24: Package manager](tooling/package-manager.md)

-   :material-cog: **Compiler Internals**

    ---

    How the compiler turns `.dream` source into WebAssembly, for contributors.

    [:octicons-arrow-right-24: Read the handbook](compiler/README.md)

</div>

## Language

<div class="grid cards" markdown>

-   :material-variable: **Basics**

    ---

    [Variables](language/variables.md) · [Operators](language/operators.md) ·
    [Control flow](language/control-flow.md) · [Functions](language/functions.md) ·
    [Comments](language/comments.md) · [Panics](language/panics.md) ·
    [Invariants](language/invariants.md)

-   :material-cube: **Types & data**

    ---

    [Overview](language/types.md) · [Primitives](language/primitives.md) ·
    [Arrays](language/arrays.md) · [Enums & unions](language/enums-unions.md) ·
    [Classes & structs](language/classes-structs.md) · [object](language/objects.md)

-   :material-puzzle: **Advanced**

    ---

    [Generics](language/generics.md) · [Interfaces](language/interfaces.md) ·
    [Async/await](language/async.md) · [WebWorkers](language/webworkers.md) ·
    [Imports & modules](language/imports.md) · [Memory](language/memory.md)

</div>

## Source generators

Compile-time code generation — no runtime reflection. Start with the overview, then pick a path:

<div class="grid cards" markdown>

-   :material-auto-fix: **Overview**

    ---

    Registration, `emit_extend` / `replace`, symbols, and checklists.

    [:octicons-arrow-right-24: Source generators](language/generators.md)

-   :material-code-tags: **Syntax DSLs**

    ---

    Custom `introducer { … }` blocks (sample: `html { }`).

    [:octicons-arrow-right-24: Syntax DSLs](language/syntax-dsls.md)

-   :material-hammer-wrench: **CodeBuilder**

    ---

    `system.codegen` helpers for building emitted Dream source.

    [:octicons-arrow-right-24: Codegen API](stdlib/codegen.md)

-   :material-code-json: **`@json` derive**

    ---

    Shipped generator: `to_json` / `from_json` for your types.

    [:octicons-arrow-right-24: JSON](stdlib/json.md)

</div>

## Standard library

<div class="grid cards" markdown>

-   :material-code-braces: **Built-ins** · `system` / bootstrap

    ---

    Console I/O (`System`), `to_string`/`hash_code`, `Buffer`, `Math`.

    [:octicons-arrow-right-24: Built-ins](stdlib/builtins.md)

-   :material-format-text: **Strings** · `system.text`

    ---

    Slicing, searching, casing, splitting, `StringBuilder`, and regex.

    [:octicons-arrow-right-24: Strings](stdlib/string.md)

-   :material-alert-circle-outline: **Option & Result** · bootstrap

    ---

    Handle missing values and errors without null.

    [:octicons-arrow-right-24: Option & Result](stdlib/option-result.md)

-   :material-layers: **Collections** · `system.collections`

    ---

    `List<T>`, `Map<K, V>`, `Set<T>`, `Queue`, `Stack`.

    [:octicons-arrow-right-24: Collections](stdlib/collections.md)

-   :material-lock: **Lock & Semaphore** · `system.sync`

    ---

    Mutual exclusion and counting semaphores for `shared` / workers.

    [:octicons-arrow-right-24: Sync](stdlib/sync.md)

-   :material-code-json: **JSON** · `system.json`

    ---

    Parse, build, and stringify JSON, plus `@json` auto-derive.

    [:octicons-arrow-right-24: JSON](stdlib/json.md)

-   :material-auto-fix: **Codegen** · `system.codegen`

    ---

    `CodeBuilder` for source generators (HTML DSL sample under `sample/generators/`).

    [:octicons-arrow-right-24: Codegen](stdlib/codegen.md)

-   :material-file-document: **File I/O** · `system.io`

    ---

    Read, write, and stream files with `File` and `FileStream`.

    [:octicons-arrow-right-24: File I/O](stdlib/file.md)

-   :material-swap-horizontal: **HTTP** · `system.net`

    ---

    Cross-runtime requests over `async`/`await` with `HttpClient`.

    [:octicons-arrow-right-24: HTTP](stdlib/http.md)

-   :material-regex: **Regex** · `system.text`

    ---

    Pattern matching and replace over strings.

    [:octicons-arrow-right-24: Regex](stdlib/regex.md)

-   :material-calendar-clock: **DateTime** · `system`

    ---

    Dates, times, arithmetic, and comparisons.

    [:octicons-arrow-right-24: DateTime](stdlib/datetime.md)

-   :material-dice-multiple: **Random** · `system`

    ---

    Pseudo-random numbers for games and tests.

    [:octicons-arrow-right-24: Random](stdlib/random.md)

-   :material-binary: **Encoding** · `system.encoding`

    ---

    Hex / base64 and related byte↔text helpers.

    [:octicons-arrow-right-24: Encoding](stdlib/encoding.md)

-   :material-text-box: **Logging** · `system.logging`

    ---

    Structured log levels and sinks.

    [:octicons-arrow-right-24: Logging](stdlib/logging.md)

-   :material-shield-key: **Crypto** · `system.crypto`

    ---

    Digests, HMAC, and secure random bytes.

    [:octicons-arrow-right-24: Crypto](stdlib/crypto.md)

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
