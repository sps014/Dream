---
hide:
  - navigation
  - toc
---

<div class="dream-hero">
  <h1 class="dream-gradient-text">Dream</h1>
  <p class="dream-hero-subtitle">
    A statically typed language that compiles to WebAssembly — Rust- and TypeScript-flavored
    syntax, ARC memory, zero-cost generics, and first-class JS interop.
  </p>
  <div class="dream-hero-actions">
    <a href="getting-started/" class="md-button md-button--primary">Get Started</a>
    <a href="https://github.com/sps014/Dream" class="md-button">GitHub</a>
  </div>
</div>

<div class="dream-code-showcase" markdown>

```dream
import system;
import system.collections;

fun main() {
    let xs = List<int>();
    xs.push(1);
    System.println(xs.len());
}
```

</div>

## Language at a glance

<div class="dream-feature-strip" markdown>

- **Static types** with inference
- **ARC** memory (no GC pauses)
- **Generics** monomorphized to WASM
- **Classes / structs / interfaces**
- **Enums & unions** with `switch`
- **`async` / `await`** in-module
- **`WebWorker`** parallelism
- **`js` + `extern`** interop
- **`@compute`** → WebGPU
- **Stdlib** collections, JSON, I/O, GPU, crypto

</div>

## Start here

<div class="dream-compact-cards" markdown>

<div class="grid cards" markdown>

-   :material-rocket-launch: **Getting Started**

    ---

    Install, write `hello.dream`, run it.

    [:octicons-arrow-right-24: Install & run](getting-started.md)

-   :material-book-open-page-variant: **Language**

    ---

    Syntax, types, async, memory, interop.

    [:octicons-arrow-right-24: Variables](language/variables.md)

-   :material-library: **Standard library**

    ---

    Collections, JSON, files, HTTP, GPU, crypto.

    [:octicons-arrow-right-24: Built-ins](stdlib/builtins.md)

-   :material-package-variant: **Tooling**

    ---

    Packages with `dreamer`.

    [:octicons-arrow-right-24: Package manager](tooling/package-manager.md)

</div>

</div>

## Language

<div class="dream-compact-cards" markdown>

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

    [Imports](language/imports.md) · [Language rules](language/invariants.md)

-   :material-puzzle: **Features**

    ---

    [Generics](language/generics.md) · [Interfaces](language/interfaces.md) ·
    [Async](language/async.md) · [WebWorkers](language/webworkers.md) ·
    [Compute](language/compute.md) · [Memory](language/memory.md)

-   :material-language-javascript: **JS interop**

    ---

    [Overview](language/interop.md) · [js type](language/js-type.md) ·
    [Callbacks](language/callbacks.md)

-   :material-auto-fix: **Metaprogramming**

    ---

    [Source generators](language/generators.md) · [CodeBuilder](stdlib/codegen.md)

</div>

</div>

## Standard library

<div class="dream-compact-cards" markdown>

<div class="grid cards" markdown>

-   :material-code-braces: **Core**

    ---

    [Built-ins](stdlib/builtins.md) · [Option & Result](stdlib/option-result.md) ·
    [Sync](stdlib/sync.md)

-   :material-format-text: **Text**

    ---

    [Strings](stdlib/string.md) · [Regex](stdlib/regex.md) ·
    [Encoding](stdlib/encoding.md)

-   :material-layers: **Collections**

    ---

    [List / Map / Set](stdlib/collections.md)

-   :material-cog: **System**

    ---

    [Random](stdlib/random.md) · [DateTime](stdlib/datetime.md) ·
    [Logging](stdlib/logging.md)

-   :material-swap-horizontal: **I/O**

    ---

    [File](stdlib/file.md) · [HTTP](stdlib/http.md)

-   :material-gpu: **GPU**

    ---

    [system.gpu](stdlib/gpu.md) · [Compute](language/compute.md)

-   :material-code-json: **JSON**

    ---

    [JSON & `@json`](stdlib/json.md)

-   :material-shield-key: **Crypto**

    ---

    [Digests & CSPRNG](stdlib/crypto.md)

</div>

</div>

## For contributors

<div class="dream-compact-cards" markdown>

<div class="grid cards" markdown>

-   :material-cog: **Contributing**

    ---

    Pipeline, IRs, passes, design notes.

    [:octicons-arrow-right-24: Handbook](compiler/README.md)

</div>

</div>
