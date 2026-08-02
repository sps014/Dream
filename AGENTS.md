# AGENTS.md — Dream Compiler

Read this fully before exploring the repo. It exists so agents don't burn tokens re-discovering structure that's already known. `docs/compiler/` is the deep-dive engineering handbook (pipeline, type system, HIR, MIR, passes, relooper, adding a feature, testing/determinism) — read it before touching the middle/back end. Everything below is the fast-reference version.

## Non-negotiable ground rules

- **No backwards compatibility.** Dream is pre-1.0 with no external users to protect. Never add shims, deprecated aliases, dual code paths, or "legacy" fallbacks to preserve old behavior. When a design changes, migrate every call site and delete the old path outright — do not leave both.
- **Prefer well-settled libraries over custom implementations.** Before hand-rolling something (arena allocation, ordered maps, tokenizing, CLI parsing, JSON, HTTP, terminal control, timezones, WASM text parsing), check `Cargo.toml` for an existing dependency, or reach for a mature crate instead of writing bespoke logic. Only hand-write something in-language when the project genuinely needs Dream-specific semantics that no crate can provide (e.g. the WAT emitter/relooper — the backend itself is intentionally custom).
- **Never panic on user input.** Lexer/parser/analyzer errors go through `DiagnosticBag`, never `panic!`/`unwrap`/`expect` on attacker- or user-controlled input.
- **The backend (`src/mir/`) only runs on validated programs.** A backend panic is an ICE (compiler bug), acceptable and expected there — but it must never be reachable from unvalidated input.
- **Determinism is non-negotiable.** Two compiles of the same source must produce byte-identical `.wat`/`.wasm`. Never iterate `std::collections::HashMap`/`HashSet` in anything that influences emitted output or its ordering — use `indexmap::IndexMap`/`IndexSet` (insertion order) or `BTreeMap` (sorted order) instead.
- **No narrating comments.** Comments explain *why* (invariants, trade-offs, non-obvious constraints), never *what* the next line does. Don't add "explaining the diff" comments.
- **Clippy is a hard gate at `-D warnings`.** Fix the root cause; don't `#[allow]` your way out except for genuine external-API constraints (with a comment saying why).

## What Dream is

A statically typed language that compiles to WebAssembly (`.wat` → `.wasm` + `.abi.json` sidecar). C-like syntax, automatic memory management via ARC (deterministic reference counting, no GC pauses), zero-cost monomorphized generics, classes/structs/interfaces/enums/discriminated unions, `Option`/`Result`, `async`/`await` with an in-module cooperative scheduler, `WebWorker` for real parallelism, JS interop (`js` type, `extern`), and a batteries-included stdlib (`List`, `Map`, `Set`, strings, JSON via `@json`, files, HTTP, regex, dates).

Rust edition 2018 (root crate) / 2021 (`dream-lsp`). Workspace resolver `"2"` so the wasm32 analyzer-only build doesn't drag in `wasmtime`.

## Repository layout

```
Dream/
├── crates/                     Front-end crates, layering enforced by the crate graph (not convention)
│   ├── dream-text/             Leaf: TextSpan, LineText, IndentedTextWriter. Depends on nothing in-workspace.
│   ├── dream-diagnostics/      DiagnosticBag, Severity, pretty source-excerpt rendering. Depends on dream-text.
│   └── dream-syntax/           Lexer (logos), AST nodes, recursive-descent parser. Depends on text + diagnostics.
│                               AST arena-allocated via bumpalo; lifetimes tied to the Bump arena.
├── src/                        Root `dream` crate — re-exports the three crates above (`crate::{syntax,diagnostics,text}`)
│   ├── main.rs                 CLI entry point (verbosity, target selection, invoking the runner)
│   ├── lib.rs                  Re-exports + exposes semantics/mir/driver APIs
│   ├── driver/                 Pipeline orchestration
│   │   ├── source_loader.rs    Resolves imports, parses multi-file programs into a ProgramAccumulator
│   │   ├── prelude.rs          Merges the embedded stdlib prelude
│   │   ├── json_derive/        Generates to_json/from_json for @json classes & discriminated unions
│   │   ├── error.rs            CompileError { Syntax, Semantic, Io } — the ONLY top-level error type
│   │   └── compiler.rs         High-level orchestrator: parse → analyze → HIR → MIR → emit → artifacts
│   ├── semantics/               Semantic analysis (type check, scope, async rules, generics)
│   │   ├── analyzer/            calls/, declarations/, expressions/, statements/, switch_unions/,
│   │   │                        hir_emit/ (lowers validated AST → typed HIR), generics.rs, type_checker.rs,
│   │   │                        await_rules.rs, js_interop.rs
│   │   └── symbol_table.rs, function_table.rs, struct_table.rs   context-tracking databases
│   ├── types/                   Structured type system (the ONLY source of truth for type identity)
│   │   ├── interner.rs          TypeInterner: hash-conses TyKind shapes to small TypeId (u32 newtype, cmp with ==)
│   │   ├── def.rs                DefTable: names nominal decls (struct/union/enum/fn) by DefId
│   │   ├── kind.rs                TyKind shapes
│   │   ├── compat.rs             Structural assignability / widening / overload resolution rules
│   │   ├── display.rs            Renders TypeId → readable name (e.g. Box<int>) — never string-key types elsewhere
│   │   └── lower.rs               Lowers AST `Type` → TypeId
│   ├── hir/                     Typed, name-resolved High-level IR — every node has a TypeId, every reference a
│   │                             resolved Binding, every call a Callee. Monomorphization = explicit instance worklist.
│   │                             Control flow still structured (if/while/for, not yet a CFG).
│   ├── mir/                     CFG-based Mid-level IR — the ONLY backend (legacy AST-walking codegen/ was deleted)
│   │   ├── lower/                Desugars HIR structured control flow into MIR basic blocks + terminators
│   │   ├── passes/                Pass manager: const_fold, algebraic, prop (copy/const-prop), sccp, gvn, dse, dce,
│   │   │                          simplify_cfg, licm, loop_unroll, sroa, tco, rc (Retain/Release insertion+elision)
│   │   ├── relooper.rs            Recovers structured block/loop/if shapes from the CFG for WASM
│   │   ├── emit/                  Lowers MIR → WAT: module.rs, protocol.rs (object protocol), release.rs, runtime.rs,
│   │   │                          strings.rs, tables.rs, types.rs/valuetype.rs/wasm_types.rs, debug_map.rs,
│   │   │                          js_marshal.rs, wat_dce.rs (structural whole-module DCE via `wast` AST, not string-matching)
│   │   ├── prune/                 Dead-code/HIR-edge pruning before lowering
│   │   ├── runtime/*.wat           Embedded runtime layers: allocator, strings, object protocol, format, async, panic
│   │   ├── abi.rs                  Heap-block tag constants
│   │   ├── build.rs                FunctionBuilder test helper for constructing MirFunctions in unit tests
│   │   ├── async_emit.rs, js_abi.rs
│   │   └── print.rs                Textual MIR dump (debugging)
│   ├── stdlib/                    Prelude .dream files + host function registration (single source of truth,
│   │   │                          embedded into the binary; both the compiler and dream-lsp reuse the exact files)
│   │   ├── mod.rs                 Registers host + inline functions; defines prelude module ordering
│   │   ├── primitives/             int, long, uint, ulong, float, double, byte, bool, char .dream extensions
│   │   ├── collections/            List, Map, Set (+ iterators), KeyValuePair
│   │   ├── core/                   Option, Result, Compare, Buffer, Bytes, Collection(+tuning), Promise, WebWorker, js
│   │   ├── text/                    string, string_iterator, regex
│   │   ├── io/                      file, file_stream
│   │   ├── net/                     http_client, http_response
│   │   ├── json/                    json, json_parser, json_value
│   │   └── system/                  console_color, datetime, debug, stopwatch, system, time
│   ├── execution/                 (feature "native") wasmtime-backed runner + host functions
│   │   ├── wasm_runner.rs           Native executor entry point
│   │   ├── host/                    console, file, http, math, memory, datetime, regex, worker — host fn impls
│   │   └── debugger/                DAP (Debug Adapter Protocol): decode, protocol, requests, runner, sourcemap, state
│   ├── intrinsics.rs               Single registry for builtins/@intrinsic-tagged stdlib ops — classify via
│   │                                IntrinsicOp::from_key/from_attributes, NEVER re-match bare strings like "print"/"len"
│   └── debug_schema.rs             Debug info schema module
├── tooling/
│   ├── dream-lsp/                 Native Rust LSP server reusing the compiler frontend (diagnostics, autocomplete,
│   │                               hover, formatting). Rust edition 2021. Depends on `dream` with default-features=false.
│   ├── dream-playground/           Browser playground (assets/, src/)
│   └── vscode/                     TypeScript VS Code extension client bundling dream-lsp
├── tests/
│   ├── cases/                      Golden `.dream` test cases + `.expected`/`.expected_error` + generated .wasm/.abi.json
│   ├── e2e_tests.rs                Compiles + runs cases with wasmtime, asserts against .expected(_error);
│   │                                includes `codegen_is_deterministic`
│   ├── mir_e2e.rs, mir_pipeline.rs MIR-focused end-to-end / pipeline tests
│   └── debugger_dap.rs             DAP debugger tests
├── docs/
│   ├── compiler/                   Engineering handbook: 01-pipeline-overview, 02-type-system, 03-hir, 04-mir,
│   │                                05-writing-passes, 06-relooper-and-backend, 07-adding-a-language-feature,
│   │                                08-testing-and-determinism, 09-nullable-purge-design-note
│   ├── language/                   User-facing language reference (variables, types, functions, generics, enums, etc.)
│   └── stdlib/                     User-facing stdlib reference (collections, string, json, http, datetime, regex, ...)
├── scripts/dap_probe.py            DAP manual-testing helper script
├── mkdocs.yml                      Docs site config (published to sps014.github.io/Dream)
└── Cargo.toml                      Workspace root
```

## The pipeline (mental model)

```
.dream source → Lexer (logos) → Parser (recursive descent, arena AST) → Semantic Analyzer
  → Typed HIR (types::TypeCtx feeds it) → MIR lowering (CFG) → Pass manager (opt passes)
  → Relooper (structured control flow recovery) → WAT emission → .wat → wat crate assembles → .wasm + .abi.json
```

Each arrow is a **total** lowering: the producer records everything the consumer needs, so the consumer never looks backward. Types are interned once (`TypeId`), so equality is `==`, never string comparison/mangling (the old `"Box_int"`-style stringly-typed system is gone — do not reintroduce string-keyed types).

| | AST (`dream-syntax`) | HIR (`src/hir`) | MIR (`src/mir`) |
|---|---|---|---|
| Shape | Tree, mirrors source | Tree, type-checked | CFG of basic blocks |
| Types | Syntactic `Type` enum | `TypeId` on every node | `TypeId` on every local |
| Names | Identifiers | Resolved `Binding`/`Callee` | `Local`/`Global` indices |
| Control flow | if/while/for/... | Same (structured) | goto/if/switch terminators |
| Generics | Type-param syntax | Explicit `MonoInstance` worklist | Already monomorphized |
| RC/alloc | Implicit | Implicit | Explicit `Retain`/`Release`/`New` |

## Crate dependency graph

```
dream-text ← dream-diagnostics ← dream-syntax ← dream (root) ← dream-lsp
```
`dream-syntax` can never reach into semantics/codegen — enforced by cargo, not convention. Root crate re-exports as `crate::syntax` / `crate::diagnostics` / `crate::text`.

## SRP boundaries (don't blur these)

- **Lexer** (`crates/dream-syntax/src/lexer.rs`): tokens only. No syntactic rules, no diagnostics assumptions.
- **Parser** (`crates/dream-syntax/src/parser/`): builds AST from tokens. No type-checking, no scope enforcement. **Recover-and-continue**: `match_token` synthesizes a placeholder + reports an error instead of bailing; `parse_program`/`parse_block` recover at declaration/statement boundaries. `parse()` *always* returns a `ProgramNode` no matter how malformed the input. Every token-consuming loop needs its `ensure_progress` guard so recovery can't spin forever. Fuzz/property tests in `crates/dream-syntax/src/tests/parser_tests.rs` (`fuzz_*`) lock in "never panics, always returns a ProgramNode" — keep green.
- **Analyzer** (`src/semantics/analyzer/`): validates types/scopes/async constraints, emits HIR. Never mutates AST structure, never generates target code.
- **Backend** (`src/mir/`): lowers typed HIR → MIR → WAT. Expects a fully validated program with resolved symbols/types. Never type-checks, never emits a compile-time diagnostic. Runs *only* after zero errors were reported.

## Error handling model

- `CompileError` (`src/driver/error.rs`) is the only top-level error enum: `Syntax` / `Semantic` (already-rendered diagnostics) / `Io`.
- User-facing problems → `DiagnosticBag::report_error("...", Some(span))`, caught during lex/parse/analyze. Never `panic!` on user input.
- **Poison type**: on a semantic error (unresolved ident, unknown call/member, ...) the analyzer reports once and returns `Type::Unknown`, which unifies with everything (`compare_data_type`, `type_str_assignable`, `overload_arg_compatible` all short-circuit on it) so one mistake doesn't cascade into a diagnostic flood. New analyzer error arms return `Type::Unknown` (never `Type::Void`) and skip further checks when an operand `is_unknown()`.
- **Backend panics are ICEs** — the one place panics are acceptable: a state the analyzer promised but the backend found violated. Never reachable from unvalidated input.

## Key single-source-of-truth registries (reuse, never re-spell)

- **Intrinsics** (`src/intrinsics.rs`): all builtin/`@intrinsic` stdlib ops. Classify via `IntrinsicOp::from_key`/`from_attributes`. Never bare-string-match `"print"`/`"len"`/`"promise_all"` in analyzer or codegen.
- **Reserved names** (`crates/dream-syntax/src/nodes/types.rs`): special member names (`constructor`/`del` via `is_special_member_name`), `@intrinsic` attribute name, synthetic for-each locals. Defined once, reused by parser/semantics/codegen.
- **Stdlib prelude** (`src/stdlib/*.dream`): single source of truth for stdlib signatures, embedded in the binary. Both the compiler and `dream-lsp` load the exact same files via `PRELUDE_FILES`. New stdlib API → define signature in the `.dream` file, wire host/inline impl in `src/stdlib/mod.rs` + codegen.

## Building, running, testing

```bash
# Build (release)
cargo build --release            # binary at target/release/dream

# Run a program
cargo run -- run path/to/file.dream        # compile + execute
cargo run -- path/to/file.dream            # compile to .wat only
cargo run -- -v run path/to/file.dream     # verbose

# Full test suite
cargo test --workspace

# Focused unit tests
cargo test -p dream types::
cargo test -p dream mir::passes::
cargo test -p dream relooper::

# LSP
cargo test -p dream-lsp
```

### VS Code extension
```bash
cd tooling/vscode
npm install
npm run compile
npx @vscode/vsce package   # produce .vsix
```

### Pre-commit / "done" gate — all three must pass
```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Testing conventions

- **Golden e2e tests** live in `tests/cases/`: add `<name>.dream`, plus either `<name>.expected` (exact stdout for successful compile+run) or `<name>.expected_error` (expected compile-time failure). Run via `cargo test`.
- **Unit tests** live next to the code they test (`types::tests`, `hir::tests`, `mir::passes::*::tests`, `relooper::tests`). Passes use `FunctionBuilder` (`src/mir/build.rs`) to build a tiny `MirFunction` and assert on the pass output.
- **Integration test** `src/mir/mod.rs::tests::hir_to_mir_to_optimized_wat` exercises HIR→MIR lowering→pass pipeline→emit in one shot — fastest signal when touching lowering/passes/emission.
- **Determinism test** `codegen_is_deterministic` (`tests/e2e_tests.rs`) compiles the same source twice and asserts byte-identical output. Never break this.

## Adding a language feature (checklist)

1. `crates/dream-syntax/src/nodes/`: add the AST representation. Let Rust's exhaustiveness checks drive you through every `match` that needs updating (parser, analyzer, HIR emit, MIR lowering).
2. `crates/dream-syntax/src/parser/`: parse it (recover-and-continue rules apply).
3. `src/semantics/analyzer/`: type-check + validate; emit HIR via `hir_emit/`.
4. `src/types/`: add/extend `TyKind` if a new type shape is needed.
5. `src/mir/lower/`: lower the new HIR shape into MIR.
6. `src/mir/emit/`: emit WAT if new runtime behavior is needed; extend `src/mir/runtime/*.wat` for new intrinsics.
7. `tests/cases/`: add a golden test (`.dream` + `.expected`/`.expected_error`).
8. If it's a stdlib API: define the signature in `src/stdlib/*.dream`, wire any host/inline logic in `src/stdlib/mod.rs`.
9. Run the full pre-commit gate above. See `docs/compiler/07-adding-a-language-feature.md` for a worked example.

## Misc conventions

- Memory: AST uses `bumpalo` arena allocation — mind lifetimes tied to the `Bump` arena.
- Avoid `unsafe` unless there's no idiomatic composition available.
- Deps of note (don't reinvent): `logos` (lexing), `bumpalo` (arena alloc), `indexmap` (deterministic maps), `wat`/`wast` (WAT text assembly + structural DCE), `wasmtime` (native execution, feature `native`), `reqwest`+`serde_json` (HTTP host fn), `crossterm` (raw terminal I/O), `chrono` (OS timezone lookups only — calendar math is hand-written in Dream itself), `tower-lsp`+`tokio`+`dashmap` (LSP server).
- `native` feature (`wasmtime`, `reqwest`, `serde_json`, `crossterm`, `chrono`) is excluded from the wasm32 analyzer-only build — keep new native-only deps behind it.
