# AGENTS.md — Dream Compiler

Read this fully before exploring the repo. It exists so agents don't burn tokens re-discovering structure that's already known. `docs/compiler/` is the deep-dive engineering handbook (pipeline, type system, HIR, MIR, passes, relooper, adding a feature, testing/determinism) — read it before touching the middle/back end. Everything below is the fast-reference version.

## Non-negotiable ground rules

- **No backwards compatibility.** Dream is pre-1.0 with no external users to protect. Never add shims, deprecated aliases, dual code paths, or "legacy" fallbacks to preserve old behavior. When a design changes, migrate every call site and delete the old path outright — do not leave both.
- **Prefer well-settled libraries over custom implementations.** Before hand-rolling something (arena allocation, ordered maps, tokenizing, CLI parsing, JSON, HTTP, terminal control, timezones, WASM text parsing), check `Cargo.toml` for an existing dependency, or reach for a mature crate instead of writing bespoke logic. Only hand-write something in-language when the project genuinely needs Dream-specific semantics that no crate can provide (e.g. the WAT emitter/relooper — the backend itself is intentionally custom).
- **Never panic on user input.** Lexer/parser/analyzer errors go through `DiagnosticBag`, never `panic!`/`unwrap`/`expect` on attacker- or user-controlled input.
- **The backend (`dream-mir`) only runs on validated programs.** A backend panic is an ICE (compiler bug), acceptable and expected there — but it must never be reachable from unvalidated input.
- **Determinism is non-negotiable.** Two compiles of the same source must produce byte-identical `.wat`/`.wasm`. Never iterate `std::collections::HashMap`/`HashSet` in anything that influences emitted output or its ordering — use `indexmap::IndexMap`/`IndexSet` (insertion order) or `BTreeMap` (sorted order) instead.
- **No narrating comments.** Comments explain *why* (invariants, trade-offs, non-obvious constraints), never *what* the next line does. Don't add "explaining the diff" comments.
- **Clippy is a hard gate at `-D warnings`.** Fix the root cause; don't `#[allow]` your way out except for genuine external-API constraints (with a comment saying why).

## What Dream is

A statically typed language that compiles to WebAssembly (`.wat` → `.wasm` + `.abi.json` sidecar). Syntax closer to Rust and TypeScript, automatic memory management via ARC (deterministic reference counting, no GC pauses), zero-cost monomorphized generics, classes/structs/interfaces/enums/discriminated unions, `Option`/`Result`, `async`/`await` with an in-module cooperative scheduler, `WebWorker` for real parallelism, JS interop (`js` type, `extern`), and a batteries-included stdlib (`List`, `Map`, `Set`, strings, JSON via `@json`, files, HTTP, regex, dates).

Rust edition 2018 (root crate) / 2021 (`dream-lsp`). Workspace resolver `"2"` so the wasm32 analyzer-only build doesn't drag in `wasmtime`.

## Repository layout

```
Dream/
├── crates/                         Layering enforced by the crate graph (not convention)
│   ├── dream-text/                 Leaf: TextSpan, LineText, IndentedTextWriter
│   ├── dream-diagnostics/          DiagnosticBag, Severity, source-excerpt rendering
│   ├── dream-syntax/               Lexer (logos), AST, recursive-descent parser (bumpalo arenas)
│   ├── dream-types/                TypeInterner, DefTable, TyKind, TypeId, compat, display, lower
│   ├── dream-hir/                  Typed HIR (Binding/Callee/TypeId); structured control flow
│   ├── dream-abi/                  Shared constants: attributes, intrinsics, JS ABI names
│   ├── dream-stdlib/               Embedded prelude .dream files + STD_PACKAGES registry
│   ├── dream-sema/                 Semantic analyzer + tables + hir_emit (fused; no MIR dep)
│   └── dream-mir/                  CFG MIR, passes, relooper, WAT emit, runtime/*.wat
├── src/                            Root `dream` crate — driver, CLI, execution only
│   ├── main.rs                     CLI entry point
│   ├── lib.rs                      Thin facade: driver + execution (+ debug_schema)
│   ├── driver/                     Pipeline orchestration (parse → analyze → HIR → MIR → emit)
│   │   ├── source_loader.rs, prelude.rs, generate/, error.rs, compiler.rs, …
│   ├── execution/                  (feature "native") wasmtime runner, host fns, DAP debugger
│   └── debug_schema.rs             Debug info schema
├── tooling/
│   ├── dream-lsp/                  LSP (depends on `dream` with default-features=false)
│   ├── dream-playground/           Browser playground
│   └── vscode/                     VS Code extension
├── tests/                          Golden e2e, MIR pipeline, DAP tests
├── docs/                           compiler/ + language/ + stdlib/
├── scripts/dap_probe.py
├── mkdocs.yml
└── Cargo.toml
```

### Module conventions

- **`mod.rs`** = module declarations + thin re-exports only (≤ ~150 LOC of logic).
- Prefer ≤ ~400 LOC production files; hard smell at ~600+.
- One concern per file. Match `dream-types`, `dream-mir` passes, and `driver/`.
- Analysis and HIR emit stay fused in `dream-sema` (intentional — do not invent a separate HIR-builder crate).
- No permanent `pub use` compatibility shims in root `dream`; consumers depend on the owning crate.

## The pipeline (mental model)

```
.dream source → Lexer (logos) → Parser (recursive descent, arena AST) → Semantic Analyzer
  → Typed HIR (types::TypeCtx feeds it) → MIR lowering (CFG) → Pass manager (opt passes)
  → Relooper (structured control flow recovery) → WAT emission → .wat → wat crate assembles → .wasm + .abi.json
```

Each arrow is a **total** lowering: the producer records everything the consumer needs, so the consumer never looks backward. Types are interned once (`TypeId`), so equality is `==`, never string comparison/mangling (the old `"Box_int"`-style stringly-typed system is gone — do not reintroduce string-keyed types).

| | AST (`dream-syntax`) | HIR (`dream-hir`) | MIR (`dream-mir`) |
|---|---|---|---|
| Shape | Tree, mirrors source | Tree, type-checked | CFG of basic blocks |
| Types | Syntactic `Type` enum | `TypeId` on every node | `TypeId` on every local |
| Names | Identifiers | Resolved `Binding`/`Callee` | `Local`/`Global` indices |
| Control flow | if/while/for/... | Same (structured) | goto/if/switch terminators |
| Generics | Type-param syntax | Explicit `MonoInstance` worklist | Already monomorphized |
| RC/alloc | Implicit | Implicit | Explicit `Retain`/`Release`/`New` |

## Crate dependency graph

```
dream-text ← dream-diagnostics ← dream-syntax
dream-types ← dream-syntax, dream-diagnostics
dream-hir ← dream-types
dream-abi ← dream-types (+ dream-syntax for attribute AST)
dream-stdlib ← dream-syntax
dream-sema ← dream-syntax, dream-types, dream-hir, dream-abi, dream-stdlib
dream-mir ← dream-hir, dream-types, dream-abi, dream-stdlib
dream (driver/CLI/execution) ← dream-sema, dream-mir, dream-stdlib, dream-abi, …
dream-lsp ← dream
```

Hard rules Cargo enforces:
- `dream-syntax` never reaches sema/MIR.
- `dream-sema` never depends on `dream-mir`.
- `dream-mir` never depends on `dream-sema`.
- Shared names (JS ABI, intrinsics, attributes) live in `dream-abi`, not in MIR.

Root `dream` may re-export front-end leaves as `dream::{syntax,diagnostics,text}` for the CLI/LSP facade; it does **not** permanently re-export middle/back-end crates.

## SRP boundaries (don't blur these)

- **Lexer** (`crates/dream-syntax/src/lexer.rs`): tokens only. No syntactic rules, no diagnostics assumptions.
- **Parser** (`crates/dream-syntax/src/parser/`): builds AST from tokens. No type-checking, no scope enforcement. **Recover-and-continue**: `match_token` synthesizes a placeholder + reports an error instead of bailing; `parse_program`/`parse_block` recover at declaration/statement boundaries. `parse()` *always* returns a `ProgramNode` no matter how malformed the input. Every token-consuming loop needs its `ensure_progress` guard so recovery can't spin forever. Fuzz/property tests in `crates/dream-syntax/src/tests/parser_tests.rs` (`fuzz_*`) lock in "never panics, always returns a ProgramNode" — keep green.
- **Analyzer** (`crates/dream-sema/`): validates types/scopes/async constraints, emits HIR. Never mutates AST structure, never generates target code (WAT/WASM).
- **Backend** (`crates/dream-mir/`): lowers typed HIR → MIR → WAT. Expects a fully validated program with resolved symbols/types. Never type-checks, never emits a compile-time diagnostic. Runs *only* after zero errors were reported.

## Backend non-goals

Do not implement these (decision record: `docs/compiler/10-stack-alloc-and-mono-design-note.md`):

- **Small-string SSO** — `string` stays a heap ARC `i32` pointer; no tagged inline representation.
- **`@stack` class-instance allocation** — classes stay heap refs; silent SROA may still promote non-escaping instances. (`@stack` on unions is shipped and unrelated.)
- **Size-class-keyed unmanaged monomorphization** — mono stays `(DefId, args)`; `unmanaged` stdlib code uses runtime `esize`, not a compiler size-class key.

Sync functions emit nested `block`/`loop`/`if` from relooper shapes; async poll functions keep `$__pc` + `br_table` (suspend/resume).

## Error handling model

- `CompileError` (`src/driver/error.rs`) is the only top-level error enum: `Syntax` / `Semantic` (already-rendered diagnostics) / `Io`.
- User-facing problems → `DiagnosticBag::report_error("...", Some(span))`, caught during lex/parse/analyze. Never `panic!` on user input.
- **Poison type**: on a semantic error (unresolved ident, unknown call/member, ...) the analyzer reports once and returns `Type::Unknown`, which unifies with everything (`compare_data_type`, `type_str_assignable`, `overload_arg_compatible` all short-circuit on it) so one mistake doesn't cascade into a diagnostic flood. New analyzer error arms return `Type::Unknown` (never `Type::Void`) and skip further checks when an operand `is_unknown()`.
- **Backend panics are ICEs** — the one place panics are acceptable: a state the analyzer promised but the backend found violated. Never reachable from unvalidated input.

## Key single-source-of-truth registries (reuse, never re-spell)

- **Intrinsics** (`crates/dream-abi/src/intrinsics.rs`): all builtin/`@intrinsic` stdlib ops. Classify via `IntrinsicOp::from_key`/`from_attributes`. Never bare-string-match `"print"`/`"len"`/`"promise_all"` in analyzer or codegen.
- **Attributes / JS ABI** (`crates/dream-abi/`): attribute helpers and JS bridge/type names shared by sema and MIR.
- **Reserved names** (`crates/dream-syntax/src/nodes/types.rs`): special member names (`constructor`/`del` via `is_special_member_name`), `@intrinsic` attribute name, synthetic for-each locals. Defined once, reused by parser/semantics/codegen.
- **Stdlib prelude** (`crates/dream-stdlib/system/*.dream`): single source of truth for stdlib signatures, embedded in the binary. Packages are registered in `STD_PACKAGES` / `BOOTSTRAP_PACKAGES` (`crates/dream-stdlib`); both the compiler and `dream-lsp` load the same files. User code imports opt-in packages (`import system.net;`, etc.); bootstrap (`system.core`, `system.primitives`) is always merged. New stdlib API → define signature in the `.dream` file, register the package file, wire host/inline logic in root `execution/` if needed.

## Building, running, testing

```bash
# Build (release)
cargo build --release            # binary at target/release/dream

# Run a program
cargo run -- run path/to/file.dream        # compile + execute (wasmtime)
cargo run -- path/to/file.dream            # compile to .wat / .wasm / .abi.json
cargo run -- -v run path/to/file.dream     # verbose

# Opt-in tree-shaken JS host next to the .wasm (browser or Node)
cargo run -- --runtime --web path/to/file.dream    # *.web.runtime.js for the browser
cargo run -- --runtime --node path/to/file.dream   # *.node.runtime.js for Node ≥ 18

# Full shared JS runtime (edit runtime/src/, then regenerate)
node scripts/bundle-runtime.mjs            # writes runtime/dream.js
node scripts/bundle-runtime.mjs --check    # fails if dream.js is stale

# Full test suite
cargo test --workspace

# Focused unit tests
cargo test -p dream-types
cargo test -p dream-mir -- passes::
cargo test -p dream-mir -- relooper::
cargo test -p dream-sema

# LSP
cargo test -p dream-lsp
```

JS interop details (full vs selective runtime): `docs/language/interop.md`.
### VS Code extension
```bash
cd tooling/vscode
npm install
npm run compile
npx @vscode/vsce package   # produce .vsix
```

For local development, point the extension at your Cargo build output instead of bundling binaries:

```bash
source ./use-toolchain.sh          # builds release dream / dream-lsp / dreamer, sets DREAM_* + PATH
# source ./use-toolchain.sh --debug
code .                             # launch VS Code from this shell so it inherits the env
```

Or set VS Code settings `dream.home` / `dreamer.home` to `<repo>/target/release` (or `target/debug`).

### Pre-commit / "done" gate — all three must pass
```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Testing conventions

- **Golden e2e tests** live in `tests/cases/`: add `<name>.dream`, plus either `<name>.expected` (exact stdout for successful compile+run) or `<name>.expected_error` (expected compile-time failure). Run via `cargo test`.
- **Unit tests** live next to the code they test (`dream-types`, `dream-hir`, `dream-mir` passes/`relooper`). Passes use `FunctionBuilder` (`dream-mir`) to build a tiny `MirFunction` and assert on the pass output.
- **Integration test** `dream-mir`'s `hir_to_mir_to_optimized_wat` exercises HIR→MIR lowering→pass pipeline→emit in one shot — fastest signal when touching lowering/passes/emission.
- **Determinism test** `codegen_is_deterministic` (`tests/e2e_tests.rs`) compiles the same source twice and asserts byte-identical output. Never break this.

## Adding a language feature (checklist)

1. `crates/dream-syntax/src/nodes/`: add the AST representation. Let Rust's exhaustiveness checks drive you through every `match` that needs updating (parser, analyzer, HIR emit, MIR lowering).
2. `crates/dream-syntax/src/parser/`: parse it (recover-and-continue rules apply).
3. `crates/dream-sema/`: type-check + validate; emit HIR via `hir_emit/`.
4. `crates/dream-types/`: add/extend `TyKind` if a new type shape is needed.
5. `crates/dream-mir/src/lower/`: lower the new HIR shape into MIR.
6. `crates/dream-mir/src/emit/`: emit WAT if new runtime behavior is needed; extend `runtime/*.wat` for new intrinsics.
7. `tests/cases/`: add a golden test (`.dream` + `.expected`/`.expected_error`).
8. If it's a stdlib API: define the signature under `crates/dream-stdlib/system/…`, register the file in `STD_PACKAGES`, wire host/inline logic in root `execution/` if needed.
9. Run the full pre-commit gate above. See `docs/compiler/07-adding-a-language-feature.md` for a worked example.

## Misc conventions

- Memory: AST uses `bumpalo` arena allocation — mind lifetimes tied to the `Bump` arena.
- Avoid `unsafe` unless there's no idiomatic composition available.
- Deps of note (don't reinvent): `logos` (lexing), `bumpalo` (arena alloc), `indexmap` (deterministic maps), `wat`/`wast` (WAT text assembly + structural DCE), `wasmtime` (native execution, feature `native`), `reqwest`+`serde_json` (HTTP host fn), `crossterm` (raw terminal I/O), `chrono` (OS timezone lookups only — calendar math is hand-written in Dream itself), `tower-lsp`+`tokio`+`dashmap` (LSP server).
- `native` feature (`wasmtime`, `reqwest`, `serde_json`, `crossterm`, `chrono`) is excluded from the wasm32 analyzer-only build — keep new native-only deps behind it.
