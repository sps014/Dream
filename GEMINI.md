# Dream Compiler & Playground — GEMINI.md

This document serves as the foundational instruction manual and architectural guide for any AI-assisted developments, refactorings, or explorations of the Dream language codebase.

> **In-depth compiler architecture docs live in [`docs/compiler/`](./docs/compiler/README.md)** — the pipeline, type system, HIR, MIR, how to write optimization passes, the relooper/backend, how to add a language feature, testing/determinism, and the migration status. Start there for the middle/back end.

---

## 1. Project Overview

**Dream** is a statically typed programming language that compiles to WebAssembly. Key features of the language include a simple C-like syntax, automatic memory management (automatic reference counting/garbage collection via a built-in object runtime and free lists), generic classes/functions, asynchronous programming via `async`/`await`, and standard collections (`List<T>` and `Map<K, V>`).

The repository is structured as a Rust-centric multi-component monorepo:
1. **`dream` (Root Crate):** The core compiler written in Rust. It compiles `.dream` source files to WebAssembly Text (`.wat`), assembles them to WASM binaries (`.wasm`), generates ABI sidecars (`.abi.json`), and provides a native runner powered by `wasmtime`.
2. **`tooling/dream-lsp` (Language Server):** A native Rust Language Server Protocol (LSP) implementation that reuses the compiler frontend to provide live diagnostics, autocomplete, hover signatures, and code-formatting to editors.
3. **`tooling/vscode` (VS Code Extension):** A TypeScript extension client that embeds the `dream-lsp` server to provide rich IDE features directly in Visual Studio Code.

---

## 2. Directory Layout & Key Modules

*   **`crates/` (Front-end crates):** The front-end is split into three layered library crates so the layering (`dream-text` <- `dream-diagnostics` <- `dream-syntax`) is enforced by the crate graph rather than convention. The root `dream` crate depends on all three and re-exports them from `lib.rs` (`pub use dream_syntax as syntax;`, etc.) so every existing `crate::{syntax,diagnostics,text}::...` path keeps resolving.
    *   `crates/dream-text/`: Leaf crate for source-position primitives (`text_span.rs`, `line_text.rs`, `indented_text_writer.rs`); depends on nothing else in the workspace.
    *   `crates/dream-diagnostics/`: Collects, stores, and pretty-prints errors and warnings with inline source excerpts and squigglies. Depends only on `dream-text`.
    *   `crates/dream-syntax/`: Lexer, AST node definitions, parser, and syntax tree. Depends on `dream-text` and `dream-diagnostics` (and `logos`/`bumpalo`).
*   **`src/` (Core Compiler):**
    *   `main.rs`: Entry point for the compiler CLI. Manages verbosity, compilation target selection, and invoking the runner.
    *   `lib.rs`: Re-exports the front-end crates (`syntax`, `diagnostics`, `text`) and exposes the semantic analyzer, codegen, and driver APIs.
    *   `driver/`: Orchestrates the compiler lifecycle.
        *   `source_loader.rs`: Recursively resolves imports, parses multiple files, and merges every file's declarations into a `ProgramAccumulator`.
        *   `prelude.rs`: Merges the embedded standard-library prelude.
        *   `json_derive.rs`: Generates `to_json`/`from_json` `extend` blocks for `@json` classes and discriminated unions.
        *   `error.rs`: Top-level `CompileError` enum returned by the pipeline.
        *   `compiler.rs`: High-level orchestrator starting with parsing and concluding with code generation and artifact emission.
    *   *Parsing stage:* lives in the `crates/dream-syntax/` crate (re-exported as `crate::syntax`). `lexer.rs` tokenizes with `logos`; `parser/` is recursive descent over declarations/statements/expressions; `nodes/` defines the AST (`ProgramNode`, `Type`, `ExpressionNode`, `StatementNode`, etc.).
    *   `semantics/`: Semantic analysis stage.
        *   `analyzer/`: Implements type check, scope validation, `async`/`await` compliance, and generic instantiation.
        *   `symbol_table.rs`, `function_table.rs`, & `struct_table.rs`: Context-tracking databases for semantic validation.
    *   *Multi-pass middle/back-end (the only backend):* a structured-types -> typed HIR -> CFG MIR -> optimization passes -> MIR->WAT pipeline. (The legacy AST-walking `codegen/` backend has been deleted; `src/mir/` is now the sole code generator.)
        *   `types/`: The structured type system. A `TypeInterner` hash-conses type shapes (`TyKind`) to compact `TypeId`s; a `DefTable` names nominal declarations by `DefId`; `compat` holds structural assignability/widening/overload rules; `display_name` renders `Box<int>`; `TypeCtx` lowers AST `Type` -> `TypeId`. Replaces stringly-typed names (`Box_int`).
        *   `hir/`: Typed, name-resolved High-level IR — every node carries a `TypeId`, every reference a resolved `Binding`, every call a `Callee`; monomorphization is an explicit instance worklist. Control flow is still structured.
        *   `mir/`: CFG-based Mid-level IR (basic blocks + terminators, explicit `Retain`/`Release`/alloc). `lower` desugars HIR control flow into the CFG; `passes/` is a pass manager with const-fold, copy/const-prop, DCE, simplify-CFG, and RC insertion/elision; `relooper` recovers structured shapes; `emit` lowers MIR to WAT. `abi.rs` holds the heap-block tag constants and `runtime/*.wat` are the embedded runtime layers (allocator, strings, object protocol, float/double format, async scheduler).
    *   `stdlib/`: Standard library implementations.
        *   `mod.rs`: Registers host and inline functions. Defines the exact ordering for standard prelude modules.
        *   `*.dream`: Standard collections (`list.dream`, `map.dream`) and primitive type extensions (`string.dream`, `int.dream`, `char.dream`, etc.).
*   **`tooling/` (Developer Tooling):**
    *   `dream-lsp/`: A native binary implementing the Language Server Protocol (LSP).
    *   `vscode/`: A TypeScript extension client for Visual Studio Code that bundles the `dream-lsp` server.
*   **`tests/` (Testing Suite):**
    *   `e2e_tests.rs`: Tests compilations, builds WASM, and runs it with wasmtime to assert outputs against `.expected` or expects failures via `.expected_error` for cases in `tests/cases/`.

---

## 3. Building, Running, and Testing

Always use standard cargo toolchain commands:

### Core Compiler

```bash
# Build compiler in release mode
cargo build --release

# Run a dream file
cargo run -- run path/to/file.dream

# Run a dream file with verbose logs
cargo run -- -v run path/to/file.dream

# Run all core compiler and integration tests
cargo test
```

### VS Code Language Server

The language service is a native Rust LSP server. The TypeScript extension client handles spawning the server locally.

```bash
# Build the LSP and compile the extension
cd tooling/vscode
npm install
npm run compile

# Package it into a .vsix for installation
npx @vscode/vsce package

# Run language service tests
cargo test -p dream-lsp
```

---

## 4. Development & Contribution Conventions

### 4.1. Engineering Principles (SOLID & DRY)
Adhere to strict software engineering standards to maintain long-term scalability and a clean compiler architecture:

*   **Single Responsibility Principle (SRP):** Keep each compilation stage or helper module strictly focused on a single task:
    *   **Lexing (`lexer.rs`):** Translates source strings into token streams. Must not embed syntactic rules or diagnostic assumptions.
    *   **Parsing (`parser/`):** Builds AST nodes from token streams. Must not evaluate type correctness or enforce binding scopes.
    *   **Semantic Analyzer (`analyzer/`):** Validates type correctness, variable scopes, and async constraints. Must not modify AST structure or introduce target code generation.
    *   **Code Generation (`mir/`):** Lowers typed HIR → MIR → target representation (`.wat`). Expects a fully validated program and resolved symbols/types; must never perform type checks or emit compile-time errors.
*   **Don't Repeat Yourself (DRY):**
    *   Consolidate common type-checking routines, helper operations, or expression evaluations into shared helper traits/methods inside `src/semantics/` or `crates/dream-syntax/src/nodes/`.
    *   The standard library files in `src/stdlib/*.dream` are the single source of truth. Both the main compiler and the `dream-lsp` reuse these exact files via `PRELUDE_FILES` to prevent behavior and definitions from drifting.
    *   **Intrinsics registry (`src/intrinsics.rs`):** the builtins/`@intrinsic`-tagged stdlib operations the compiler special-cases live in one place. Recognize them through the registry's constants/predicates and classify `@intrinsic("…")` static methods via `IntrinsicOp::from_key`/`from_attributes` — never re-match bare strings like `"print"`, `"len"`, or `"promise_all"` in the analyzer or codegen.
    *   **Reserved names (`crates/dream-syntax/src/nodes/types.rs`):** special member names (`constructor`/`del` via `is_special_member_name`), the `@intrinsic` attribute name, and synthetic `for-each` locals are defined once and reused by parser/semantics/codegen rather than re-spelled as literals.
*   **Open/Closed Principle (OCP):**
    *   Compiler passes rely on robust pattern matching over abstract syntax enums (e.g., `ExpressionNode` or `StatementNode`).
    *   When adding a new statement or expression, declare its representation in `crates/dream-syntax/src/nodes/` and let the Rust compiler's exhaustiveness checks guide you through updating the matching blocks across the parser, analyzer, and codegen. This design allows extending the language safely with compile-time correctness guarantees.
*   **Interface Segregation & Loose Coupling:**
    *   The core compilation workflow (`Compiler`) is decoupled from runtime host integration. Native execution details remain isolated in `src/execution/host.rs`, and browser playground details are separated into JS runtime wrappers within Vite.

### 4.2. General Code Quality & Tooling
*   **Rust Standards:** Prioritize compiling with standard warnings and formatting with `cargo fmt`. Avoid `unsafe` or complex raw memory manipulation when idiomatic composition is possible.
*   **Memory Management:** The compiler heavily relies on the `bumpalo` arena allocator for AST node allocations, optimizing memory operations and parsing speeds. Be mindful of lifetimes (`'a`) linked to the `Bump` arena.

### 4.3. Diagnostic & Error Reporting
*   Never panic on syntax or type errors inside compilation steps. Instead, report errors and warnings to `DiagnosticBag` to enable graceful error recovery and nice formatting output in Monaco Editor/CLI:
    ```rust
    diagnostics.report_error("Message text".to_string(), Some(node_span));
    ```
*   **Parser is recover-and-continue.** `match_token` synthesizes a placeholder token (and reports an error) instead of bailing, and `parse_program`/`parse_block` recover at declaration/statement boundaries, so `parse()` *always* returns a `ProgramNode` regardless of how malformed the input is. Every token-consuming loop must keep its `ensure_progress` guard so recovery can never spin forever. The fuzz/property tests in `crates/dream-syntax/src/tests/parser_tests.rs` (`fuzz_*`) lock in the "never panics, always returns a ProgramNode" guarantee — keep them green when touching the parser.
*   **Semantics use a poison type to stop cascades.** On a type error (unresolved identifier, unknown call/member, etc.) the analyzer reports once and returns `Type::Unknown`. `Unknown` unifies with every type (`compare_data_type`, `type_str_assignable`, `overload_arg_compatible` all short-circuit on it), so a single mistake never snowballs into a flood of follow-on diagnostics. New analyzer arms should return `Type::Unknown` on error (not `Type::Void`) and skip their own checks when an operand `is_unknown()`. Codegen never runs once any error is reported, so `Unknown` never needs lowering.

### 4.4. Standard Library (Prelude)
*   The standard library files under `src/stdlib/*.dream` are embedded directly into the compiled binary.
*   Whenever a new type method or core standard API is introduced, define its signature in the corresponding `.dream` prelude file, then implement any inline runtime or host backend inside `src/stdlib/mod.rs` and the codegen system.

### 4.5. Extending Tests
*   When fixing a bug or adding a feature, write a corresponding test case in `tests/cases/`.
*   **Golden Tests Workflow:**
    *   Create `tests/cases/your_feature.dream`.
    *   If expected to compile and run successfully, create `tests/cases/your_feature.expected` containing the exact standard output of the program execution.
    *   If expected to fail compile-time validation, create `tests/cases/your_feature.expected_error`.
    *   Run `cargo test` to execute your test cases.
