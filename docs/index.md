---
hide:
  - navigation
  - toc
---

<div class="dream-hero">
  <h1 class="dream-gradient-text">Dream</h1>
  <p class="dream-hero-subtitle">
    A fast, statically typed language that compiles to WebAssembly — TS/Rust-like syntax,
    automatic memory management, and a batteries-included standard library.
  </p>
  <div class="dream-hero-actions">
    <a href="getting-started/" class="md-button md-button--primary">Get Started</a>
    <a href="https://github.com/sps014/Dream" class="md-button">GitHub</a>
  </div>
</div>

<div class="dream-code-showcase" markdown>

```dream
import system;

fun greet(name: string): string {
    return "Hello, " + name;
}

fun main() {
    System.println(greet("world"));
}
```

</div>

## Start here

<div class="grid cards" markdown>

-   :material-rocket-launch: **Getting Started**

    ---

    Install the compiler, write your first program, and run it.

    [:octicons-arrow-right-24: Install & run](getting-started.md)

-   :material-book-open-page-variant: **Language**

    ---

    Syntax, types, async, memory, interop, and source generators.

    [:octicons-arrow-right-24: Variables](language/variables.md)

-   :material-library: **Standard library**

    ---

    Collections, JSON, files, HTTP, crypto, and more.

    [:octicons-arrow-right-24: Built-ins](stdlib/builtins.md)

-   :material-package-variant: **Tooling**

    ---

    Publish and consume libraries with `dreamer`.

    [:octicons-arrow-right-24: Package manager](tooling/package-manager.md)

</div>

## Language

<div class="grid cards" markdown>

-   :material-variable: **Basics**

    ---

    [Variables](language/variables.md) · [Operators](language/operators.md) ·
    [Control flow](language/control-flow.md) · [Functions](language/functions.md) ·
    [Comments](language/comments.md) · [Panics](language/panics.md)

-   :material-cube: **Types**

    ---

    [Overview](language/types.md) · [Primitives](language/primitives.md) ·
    [Arrays](language/arrays.md) · [Enums & unions](language/enums-unions.md) ·
    [Classes & structs](language/classes-structs.md) · [object](language/objects.md)

-   :material-folder-outline: **Structure**

    ---

    [Imports & modules](language/imports.md) · [Language rules](language/invariants.md)

-   :material-puzzle: **Features**

    ---

    [Generics](language/generics.md) · [Interfaces](language/interfaces.md) ·
    [Async/await](language/async.md) · [WebWorkers](language/webworkers.md) ·
    [Memory](language/memory.md)

-   :material-javascript: **Interop**

    ---

    [Overview](language/interop.md) · [js type](language/js-type.md) ·
    [Callbacks](language/callbacks.md)

-   :material-auto-fix: **Metaprogramming**

    ---

    Compile-time derives and custom syntax DSLs.

    [:octicons-arrow-right-24: Source generators](language/generators.md) ·
    [CodeBuilder](stdlib/codegen.md)

</div>

## Standard library

<div class="grid cards" markdown>

-   :material-code-braces: **Core**

    ---

    [Built-ins](stdlib/builtins.md) · [Option & Result](stdlib/option-result.md) ·
    [Lock & Semaphore](stdlib/sync.md)

-   :material-format-text: **Text & data**

    ---

    [Strings](stdlib/string.md) · [Regex](stdlib/regex.md) ·
    [Encoding](stdlib/encoding.md)

-   :material-layers: **Collections**

    ---

    `List`, `Map`, `Set`, `Queue`, `Stack`.

    [:octicons-arrow-right-24: Collections](stdlib/collections.md)

-   :material-cog: **System**

    ---

    [Random](stdlib/random.md) · [DateTime](stdlib/datetime.md) ·
    [Logging](stdlib/logging.md)

-   :material-swap-horizontal: **I/O**

    ---

    [File I/O](stdlib/file.md) · [HTTP](stdlib/http.md)

-   :material-code-json: **Serialization**

    ---

    JSON parse/stringify and `@json` auto-derive.

    [:octicons-arrow-right-24: JSON](stdlib/json.md)

-   :material-shield-key: **Security**

    ---

    Digests, HMAC, and secure random bytes.

    [:octicons-arrow-right-24: Crypto](stdlib/crypto.md)

</div>

## For contributors

<div class="grid cards" markdown>

-   :material-cog: **Contributing**

    ---

    Compiler handbook: pipeline, IRs, passes, and design notes.

    [:octicons-arrow-right-24: Handbook](compiler/README.md)

</div>
