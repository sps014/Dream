# Design note: rejected — SSO, `@stack` class instances, size-class mono

Decision record for three optimizations that were scoped out of the value-unions /
`ref struct` / `Span<T>` / `Pointer<T>` work as "design now, implement later." They are now
**permanent non-goals**: do not implement them, and do not leave half-landed ABI or mono-key
paths for them. See `09-nullable-purge-design-note.md` for the sibling record on `Option<T>`
boxing. Backend non-goals are also summarized in `AGENTS.md`.

## 1. Small-string inline (SSO) representation — **rejected**

**Problem (historical):** every `string`, however short, is a heap allocation with an ARC header.
For string-heavy code most strings are short-lived and short, so allocate/retain/release traffic
dominates character data.

**Proposed representation (never built):** a tagged value with inline ≤15-byte UTF-8 vs boxed
heap (today's layout), mirroring value unions' inline/box split.

**Why rejected:** `string` is a first-class primitive threaded through the type system, runtime
(`src/mir/runtime/strings.wat`), every emitter path that assumes `TyKind::Prim(PrimTy::String)` is
an `i32` pointer, RC insertion (`interner.is_reference(ty)` is purely type-driven), JS marshaling,
and the debugger. An `(i32, i64)` ABI and per-value heap/inline checks are closer to a language
ABI change than a self-contained follow-up. Heap-pointer strings stay the model:
`PrimTy::String` remains `is_reference() == true`.

## 2. Opt-in `@stack` class-instance stack allocation — **rejected**

**What exists and stays:** `src/mir/passes/sroa.rs` silently promotes non-escaping,
default-constructed class instances' fields to scalar locals. `@stack` on **discriminated
unions** (checked inline contract) is shipped and unchanged — see `docs/language/enums-unions.md`.

**Why rejected as a user-facing class feature:** a diagnosable "this `new` must not escape"
guarantee needs expression-level syntax (attributes are declaration-only today) plus HIR-level
escape analysis (the backend cannot emit diagnostics). That is a new language contract, not a
small emitter tweak. Classes remain heap reference types; silent SROA is the only stack-like
optimization for instances.

## 3. Size-class-keyed monomorphization for `unmanaged` generics — **rejected**

**Problem (historical):** `T: unmanaged` generics get one WAT body per concrete `T`. Bodies that
only differ by size could theoretically share codegen keyed by size class.

**Why rejected:** Dream's `unmanaged`-shaped stdlib (`Pointer<T>`, `Span<T>`, buffers) already
avoids bloat by computing offsets from a **runtime** element size (`scalar_size` / `esize` in
`src/mir/emit/emitter/rvalue/mod.rs`), not monomorphization-time field offsets. Monomorphization
stays nominal: `(DefId, Vec<TypeId>)` via `MonoInstance`. No compiler size-class key.

**Stdlib authoring rule:** write `unmanaged`-generic code so every `T`-sized access goes through
a runtime size; do not expect a compiler pass to merge same-sized instantiations.
