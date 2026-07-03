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

<div class="grid cards" markdown>

-   :material-shield-check: **Statically Typed**

    ---

    Errors caught at compile time. Safe, robust, and reliable typing system.

-   :material-web: **WebAssembly Native**

    ---

    Compiles to a `.wat` file for any WASM runtime. Highly portable and fast execution.

-   :material-memory: **Automatic Memory**

    ---

    Deterministic, fast automatic reference counting (ARC). No garbage collector pauses, no manual frees.

-   :material-shape: **Generics**

    ---

    Write highly reusable code without performance loss. Zero-overhead abstractions.

-   :material-library: **Built-in Collections**

    ---

    High-performance standard `List<T>` and `Map<K, V>` structures are built right in.

</div>

## Start Here

<div class="grid cards" markdown>

-   :material-rocket-launch: **Getting Started**

    ---

    New to Dream? Follow our quick installation and setup guide. Learn how to write your first program and run it.

    [:octicons-arrow-right-24: Install & Run](getting-started.md)

-   :material-book-open-page-variant: **Language Basics**

    ---

    Already know the basics? Jump straight into the syntax documentation covering variables, operators, and control flow.

    [:octicons-arrow-right-24: Learn Syntax](language/variables.md)

</div>

## Standard Library

<div class="grid cards" markdown>

-   :material-code-braces: **Built-ins**

    ---

    Essential core utilities: `print`, `println`, `x.to_string()`, `x.hash_code()`, `Array.new`, `Math.*`

    [:octicons-arrow-right-24: View Built-ins](stdlib/builtins.md)

-   :material-format-text: **Strings**

    ---

    Rich string manipulations: `substring`, `contains`, `trim`, `to_lower`, and more.

    [:octicons-arrow-right-24: String Docs](stdlib/string.md)

-   :material-numeric: **Primitives**

    ---

    Core scalar types including `int`, `float`, `bool`, `char`, `byte`, `long`, etc.

    [:octicons-arrow-right-24: Primitive Types](language/primitives.md)

-   :material-alert-circle-outline: **Option & Result**

    ---

    Safe monadic handling of missing values and errors, preventing null-pointer reference issues.

    [:octicons-arrow-right-24: Option & Result Docs](stdlib/option-result.md)

-   :material-layers: **Collections**

    ---

    Power your algorithms with robust implementations of `List<T>` and `Map<K, V>`.

    [:octicons-arrow-right-24: Collections Docs](stdlib/collections.md)

-   :material-code-json: **JSON**

    ---

    Structured JSON manipulation: `JsonValue` model, `JSON.parse`/`stringify`, and `@json` auto-derive support.

    [:octicons-arrow-right-24: JSON Docs](stdlib/json.md)

-   :material-file-document: **File I/O**

    ---

    Interact with the filesystem via `File` and `FileStream`. Read, write, list, and stream bytes/text.

    [:octicons-arrow-right-24: File I/O Docs](stdlib/file.md)

-   :material-swap-horizontal: **HTTP**

    ---

    Cross-runtime request execution over standard `async`/`await` powered by `HttpClient`.

    [:octicons-arrow-right-24: HTTP Docs](stdlib/http.md)

-   :material-calendar-clock: **DateTime**

    ---

    Flexible calendar date and time tracking: instantiation, arithmetic, and comparison operations.

    [:octicons-arrow-right-24: DateTime Docs](stdlib/datetime.md)

</div>

## Interop

Dream runs natively in the browser, Node.js, and native WASM runtimes. The `extern` keyword bridges seamlessly to the JavaScript host with no boilerplate.

<div class="grid cards" markdown>

-   :material-javascript: **JS Interop**

    ---

    Call JavaScript functions directly from Dream and leverage Web API ecosystems.

    [:octicons-arrow-right-24: Interop Overview](language/interop.md)

-   :material-link: **The `js` Type**

    ---

    Read, call, and mutate any live JavaScript value with native syntax — no fixed-arity helpers.

    [:octicons-arrow-right-24: The js type](language/references.md)

-   :material-phone-in-talk: **Callbacks**

    ---

    Pass Dream functions into JS and call JS functions from Dream, in either direction.

    [:octicons-arrow-right-24: Callback Docs](language/callbacks.md)

</div>
