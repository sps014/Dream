# Design note: small-string SSO, `@stack` class instances, and unmanaged-generic monomorphization

A decision record for three optimizations scoped out of the value-unions/`ref struct`/`Span<T>`/
`Pointer<T>` plan (see `09-nullable-purge-design-note.md` for the sibling record on `Option<T>`
boxing) as "design now, implement later" — each is either a genuinely separate, self-contained
follow-up, or blocked on an architectural decision that shouldn't be made under the same PR as the
plan that motivated it.

## 1. Small-string inline (SSO) representation

**Problem:** every `string`, however short, is a heap allocation with an ARC header. For
string-heavy code (JSON, regex, parsing, templating) most strings are short-lived and short
(`"true"`, `","`, a single JSON key), so the allocate/retain/release traffic dominates over the
actual character data.

**Proposed representation:** a `string` becomes a small tagged value, mirroring how
[value unions](../language/enums-unions.md) already give a discriminated union either an inline or
a boxed representation depending on payload shape:

- **Inline (≤ 15 bytes UTF-8):** the string's bytes are stored directly in the value's own storage
  (a local, a struct field, a stack slot) alongside a length byte — no heap block, no ARC header, no
  `retain`/`release` traffic at all for that value.
- **Boxed (> 15 bytes):** falls back to today's representation unchanged (heap block + ARC header +
  `char[]`-shaped payload).

This is the same "small inline, else box" shape `Option<T>`/value unions already use, and
mirrors the industry-standard SSO used by `std::string`/Rust's (unstable) small-string crates/Swift's
`String`.

**Why this is a separate decision record, not a value-union special case:** unlike a value union,
`string` is used *pervasively* as a first-class primitive throughout the type system, the runtime
(`src/mir/runtime/strings.wat`), and every emitter path that touches `TyKind::Prim(PrimTy::String)`
directly (not via the generic value/reference dispatch value unions go through). Concretely, this
touches:

- `src/mir/runtime/strings.wat` — every string primitive (`$string_concat`, `$string_eq`,
  `$string_substring`, `$char_at`, `$string_to_bytes`, ...) needs an inline/boxed dispatch instead of
  always dereferencing a heap pointer.
- `src/mir/emit/strings.rs`, `emitter/rvalue/mod.rs`, `emitter/casts.rs` — every `TyKind::Prim(PrimTy::String)`
  load/store/compare currently assumes "a string is an i32 pointer."
- `src/mir/passes/rc.rs` — an inline string needs *no* `Retain`/`Release` at all (like a plain `int`),
  so the RC-insertion pass needs a per-value (not just per-type) "is this actually heap-backed" check,
  which today is purely type-driven (`interner.is_reference(ty)`).
- JS interop (`js_marshal.rs`, `js_abi.rs`) — a string crossing the host boundary needs the same
  inline/boxed dispatch on the marshaling side.
- The debugger (`debug_map.rs`, DAP `sourcemap`) — a string value's runtime representation changes
  shape, so debug rendering needs updating too.

That is a materially larger blast radius than a single MIR pass or a single stdlib type — closer in
shape to the nullable-purge or `ref struct` work than to a self-contained follow-up. It deserves its
own implementation plan (and its own golden-test sweep across every stdlib string-touching API)
rather than being folded into this one.

**Recommended representation sketch, for the follow-up that implements this:**

```
i32 tagged value, low bit as the discriminant:
  bit0 = 0: boxed  — upper 31 bits are a heap pointer, exactly today's representation.
  bit0 = 1: inline — remaining bits hold (length: 4 bits, up to 15) + up to 15 bytes packed into a
            second i64 slot (so the value is really an (i32, i64) pair at the ABI level, matching how
            a value struct with two scalar fields is already passed/returned).
```

This keeps the boxed case byte-identical to today (so no `strings.wat` primitive needs to change for
the boxed path — only a length-checked branch added at each entry point) and confines the new
inline-path logic to the handful of primitives that actually inspect string bytes.

**Not decided yet (left to the implementation PR):** the exact inline capacity/tag encoding, whether
`char_at`/indexing needs a different codegen shape for the inline case, and whether the ABI-level
shape change (`i32` → `(i32, i64)`) is worth its own JS-interop marshaling cost for a currently-`i32`
`string` parameter/return.

## 2. Opt-in `@stack` escape-analysis-driven stack allocation for class instances

**What exists today, already:** `src/mir/passes/sroa.rs` already promotes a non-escaping,
default-constructed class instance's fields to plain scalar locals — the allocation disappears
entirely, with zero attribute or opt-in needed. This is a real precedent for "prove an instance never
escapes its creating function, then avoid the heap allocation" — it just currently applies silently,
whole-function, at the MIR level, with no user-visible attribute and no user-visible failure mode.

**Why an opt-in `@stack` on class instances is *not* implemented as part of this plan:** the natural
shape for the feature ("`@stack` on a `new Foo(...)` call site; error if escape analysis can't prove
it doesn't escape") runs into two structural walls that both need a real decision, not just an
implementation:

1. **Attributes are declaration-only.** Every existing `@attr` in `src/attributes.rs`
   (`AttributeTarget::{Function,Method,StaticMethod,ExternFunction,Field,Struct,ValueStruct,...}`)
   annotates a *declaration*. `@stack` on a discriminated union annotates the `union` declaration;
   `ref struct` annotates the `struct` declaration. A per-call-site `@stack` on `new Foo(...)` would
   be the first *expression*-level attribute in the language — new grammar (`crates/dream-syntax`),
   a new `AttributeTarget::Expression` (or a dedicated syntax entirely, e.g. `stack new Foo(...)`,
   closer to how `ref struct` reads), and new plumbing through every AST → HIR stage that currently
   assumes attributes live only on declarations.
2. **The backend cannot emit diagnostics.** Per this repo's SRP boundary (`AGENTS.md`: "the backend
   ... never emits a compile-time diagnostic ... runs *only* after zero errors were reported"),
   `sroa.rs` — a MIR pass — is architecturally the wrong place to hard-error "this `@stack` instance
   escaped." Escape analysis precise enough to *promise* an error (not just opportunistically
   optimize) has to run in `src/semantics/analyzer/` on the *typed HIR*, mirroring exactly the
   `ref struct` escape analysis already built in `src/semantics/analyzer/declarations/structs.rs` —
   which checks "does this specific value's use ever look like a field store / generic argument /
   closure capture / async parameter" using the same conservative rule set already validated for
   `ref struct`. Reusing the MIR-level SROA analysis for a *user-facing guarantee* would mean either
   duplicating that HIR-level analysis anyway (to have something diagnosable pre-backend) or
   threading a "SROA failed to promote this specific site" signal backward from MIR into a
   diagnostic — which the backend boundary rule above forecloses.

**Recommended direction for the follow-up implementation:** don't reuse `sroa.rs` at all for the
diagnosable guarantee. Instead:

- Reuse the *escape-check predicates* already written for `ref struct`
  (`src/semantics/analyzer/declarations/structs.rs`: rejected as a field type, a generic type
  argument, a lambda capture, an `async` parameter) as a shared helper, applied to a *value* (the
  result of one `new Foo(...)` call) rather than to every value of a *type*.
- Introduce the attribute as call-site syntax analogous to `ref struct`'s declaration-site keyword
  rather than a general expression-attribute mechanism — e.g. a `stack` keyword prefixing the `new`
  expression (`stack Foo(...)`) — so the grammar addition is narrowly scoped to one call-expression
  shape, not "attributes now apply to expressions in general."
- On success (the analyzer proves no escape), lower to a HIR node the existing `sroa.rs` machinery
  already knows how to fully eliminate (today's `New { ctor: None }`-and-non-escaping shape it already
  promotes) — so *no new backend code* is needed at all; the semantic analyzer's job is solely to
  make the existing optimization a checked guarantee instead of a best-effort one, and to reject the
  program if the guarantee can't be proven.
- Extending `sroa.rs`'s classification (today: field stores/loads only) to also handle a constructor
  call with arguments (not just the default zero-init constructor) is a prerequisite either way, since
  the common case (`stack Point(x, y)`) is not what `sroa.rs` promotes today.

This is scoped as its own follow-up rather than folded in here because it is a new *user-facing
contract* (a program that fails to compile if the guarantee can't be proven), which deserves its own
design review of the exact escape rules and syntax, not a rushed addition alongside the unrelated
`Span<T>`/`Pointer<T>`/`@unsafe` work.

## 3. Size-class-keyed monomorphization for `unmanaged`-constrained generics

**Problem:** a function generic over `T: unmanaged` (blittable — recursively free of reference
fields, the same shape `Pointer<T>`/`Span<T>` already require) gets one full monomorphized body per
concrete `T` today, same as any other generic function. For a large call surface (e.g. a hypothetical
`unmanaged`-constrained sort/serialize/hash utility instantiated over many small structs), this is
pure code-size bloat: the generated WAT for `foo<Vec2>` and `foo<Point3D>` (both 12-byte blittable
value structs) is byte-for-byte identical except for field-offset constants baked into field
accesses — the *shape* the codegen produces is a function of `T`'s size (and, if the body does field
access, its layout), not its nominal identity.

**Exploration, not a committed design:** unlike the two records above, this genuinely needs
profiling data before a design is worth writing down in detail — monomorphization bloat is a
real-world problem in exactly this shape (C++ template bloat, Rust monomorphization bloat), but
Dream's generics are used far less pervasively over `unmanaged` types today (only `Pointer<T>`/
`Span<T>` are `unmanaged`-shaped, and neither one currently has a size-class-sensitive body — both
are already "one accessor per element, offset computed at the call site via `esize`," not
per-instantiation-baked offsets) for this to be worth speculatively implementing. Recorded here as a
placeholder so future generic-heavy `unmanaged` APIs know the lever exists:

- **The key rule that would make this sound:** a size-class-keyed shared body is only valid for a
  generic function whose *only* interaction with `T` is through its size (`sizeof(T)`), never through
  a per-field access with a `T`-specific offset baked in as a compile-time constant. `Buffer.alloc<T>`/
  `Buffer.realloc<T>`/`Pointer<T>`'s own methods already satisfy this (every element access already
  computes its offset from a runtime `esize` parameter — see `scalar_size` in
  `src/mir/emit/emitter/rvalue/mod.rs` — never a monomorphization-time constant), which is exactly why
  they don't need this optimization to already avoid bloat: they were written size-class-generic from
  the start, at the source level, not via a compiler transform.
- **Implication:** the actual lever isn't a compiler pass at all for functions shaped like today's
  `Buffer`/`Pointer<T>` — it is a *documented stdlib-authoring pattern* ("write `unmanaged`-generic
  code so every access goes through a runtime size, and it is automatically size-class-shared in
  spirit, one instantiation but zero extra bloat per size"). A real compiler-level size-class monomorphization
  pass only earns its complexity once a generic API's body *needs* field-specific offsets (e.g. a
  future `unmanaged`-constrained hashing/equality utility that walks `T`'s fields), which does not
  exist in the stdlib today.
