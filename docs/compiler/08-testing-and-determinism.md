# 08 — Testing, Determinism & Conventions

This chapter covers how the compiler is tested, the determinism contract the whole back end must honor, and the conventions every contributor is expected to follow.

## The test pyramid

```mermaid
flowchart TD
    unit["Unit tests (per module)\ntypes::tests, hir::tests, mir::passes::*::tests, relooper::tests"]
    integ["Integration tests\nmir hir_to_mir_to_optimized_wat (HIR→MIR→passes→WAT)"]
    e2e["End-to-end tests (tests/)\ncompile a .dream program, run it, check output"]
    det["Determinism test\ncodegen_is_deterministic"]
    unit --> integ --> e2e
    e2e --- det
```

### Unit tests

Each module tests its own logic with the smallest possible input. Passes use `FunctionBuilder` (`src/mir/build.rs`) to construct a tiny `MirFunction`, run the pass, and assert on the result. The type system tests interning, nullable collapsing, reference classification, display, and compat. Run a focused subset with a path filter:

```bash
cargo test -p dream types::
cargo test -p dream mir::passes::
cargo test -p dream relooper::
```

### Integration test

`src/mir/mod.rs::tests::hir_to_mir_to_optimized_wat` exercises the whole middle/back end in one shot: build typed HIR by hand → `lower_function` → `PassManager::default_pipeline` → `emit`. When you change lowering, passes, or emission, this is the fastest signal that the stages still compose.

### End-to-end tests — `tests/`

`tests/e2e_tests.rs` and `tests/mir_e2e.rs` compile real `.dream` programs through the full driver and check behavior against each case's `.expected`. These are the source of truth for "does the compiler actually work"; every case in `tests/cases/` must pass end-to-end through the MIR backend.

### Determinism test — `codegen_is_deterministic` (`tests/e2e_tests.rs`)

Compiles the same input twice and asserts byte-identical output. This guards the contract below.

## The determinism contract

> **Two compilations of the same source must produce byte-identical `.wat`/`.wasm`.**

This is non-negotiable: it makes builds reproducible, caching sound, and diffs meaningful. The only realistic way to break it is **iteration order of a hash map**. Rules:

- **Never iterate `std::collections::HashMap`** in any code that influences emission (or its ordering).
- Use `indexmap::IndexMap` when you need insertion-order iteration with hash lookup, or `BTreeMap` when you need sorted iteration. The emission-driving maps (struct/union/enum/symbol tables, codegen string/function/global maps) already standardize on `IndexMap`.
- The `TypeInterner` assigns ids in first-seen order and stores them in a `Vec`, so iterating types by `TypeId` is deterministic.
- When you add a lookup structure to a pass or the emitter, pick `IndexMap`/`BTreeMap` deliberately, and extend a determinism assertion if it feeds output.

```mermaid
flowchart LR
    hm["std::HashMap iteration"] -->|random order per run| nondet["non-deterministic output"]
    im["IndexMap / BTreeMap / Vec"] -->|stable order| det["byte-identical output"]
```

## The pre-commit gate

Before considering any change done, all three must pass:

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Clippy runs with `-D warnings`: the project's stance is **fix the root cause, do not `#[allow]`**. The only surviving `#[allow]`s annotate *external* API constraints (e.g. an `lsp-types` field deprecated upstream) and carry a comment explaining why.

## Coding conventions

### Comments

Comments explain **intent, invariants, and trade-offs** — the *why*. They must not narrate the code. Delete `// increment counter` and `/// Builds X` stub banners. Good comments look like the module headers in `src/mir/mod.rs` (what the IR guarantees) or the back-edge note in `relooper.rs` (a subtle correctness reason).

### Errors

The driver's only typed error is `CompileError` (`src/driver/error.rs`): `Syntax` / `Semantic` (both already rendered as diagnostics) and `Io`. The back end itself has **no user-facing error path**: it runs only on a fully validated program with resolved symbols and types.

- **User-facing problems** are caught earlier as diagnostics (lex/parse/analyze). The backend never emits a compile-time error.
- **Backend invariant violations** are compiler bugs (ICE): `panic!` with a clear message. This is the one place panics are acceptable — a state the analyzer promised but the backend found violated.

### No pre-release back-compat

The compiler is unreleased. Do **not** add deprecation shims, re-export facades, or "keep the old path working" layers. When you replace something, delete the old thing (exactly what happened when the MIR backend replaced the legacy AST-walking codegen). Back-compat is debt we have not earned yet.

### Determinism by default

Reach for `IndexMap`/`BTreeMap`/`Vec` first. Only use `HashMap` for throwaway local computations whose iteration order never escapes into output.

## A good change, end to end

```mermaid
flowchart LR
    w[write code] --> u[add focused unit test]
    u --> b[cargo build]
    b --> c["cargo clippy -D warnings"]
    c --> t[cargo test --workspace]
    t --> d[update this handbook / GEMINI.md if architecture changed]
```
