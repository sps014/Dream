# Design note: purging `T?` in favor of `Option<T>`

This is a decision record, not a tutorial. It exists to settle three questions *before* touching
the ~50-file blast radius of removing `TyKind::Nullable` (see
[07 — Adding a Feature](./07-adding-a-language-feature.md) for the general shape of a
pipeline-wide change, and the redundancy-audit plan for the full file inventory). Once these three
decisions are accepted, `removal-nullable-implementation` is mechanical: delete `TyKind::Nullable`
and its ~5 core `TypeInterner` methods, then follow the compiler errors through every `strip_nullable`
call site.

## 1. What does `null` become?

**Decision: the `null` literal is removed from the language entirely.** `None` (the existing
`Option<T>` variant constructor) becomes the sole way to spell "no value."

Rationale: keeping `null` as sugar for `Option.None` would recreate exactly the redundancy this
purge exists to remove — two spellings (`null`, `None`) for one concept, which is the same
complaint leveled at `Int32`/`int` and `Array<T>`/`List<T>` elsewhere in the audit. A language that
just deleted its second collection type and its second primitive-naming system should not grow a
second "absence" literal to replace the one it removed.

Concretely:
- The `null` keyword/token is deleted from the lexer and parser (`crates/dream-syntax`), not just
  its type. `Type::Nullable(Void)`/`is_null_literal` in `src/types/compat.rs` and the parser's
  null-literal production (`crates/dream-syntax/src/parser/expressions.rs`) go away together.
- Every former `T?` field/variable becomes `Option<T>`, initialized with `None` and read via
  `switch`/`.unwrap_or(...)`/`.is_some()` — the same API the stdlib's `Map`/`List` already expose,
  per the audit's confirmation that stdlib internals never used `T?` in the first place.
- Class fields that were `T?` with an implicit "defaults to null" now must be explicit:
  `field: Option<T> = None;` (no implicit default-initialization gap — this is a small, deliberate
  strictness increase, not a regression, since implicit-null fields are exactly the class of bug
  `Option<T>` exists to prevent).

## 2. Is `??` repurposed for `Option<T>`, or does it disappear?

**Decision: `??` is repurposed as sugar for `Option<T>.unwrap_or(...)`, not removed.**

`expr ?? default` type-checks when `expr : Option<T>` and `default : T`, and lowers directly to
`expr.unwrap_or(default)` at HIR-emission time (a pure desugaring, same tier as the existing
`$"..."` → `+`-chain sugar) rather than as a new MIR shape. This keeps the ergonomic win `??`
already provided (a short-circuiting default expression, useful inline in the middle of a larger
expression where a `switch` statement can't go) without inventing a second, competing "unwrap with
default" spelling next to `.unwrap_or(...)`.

Why not drop `??` outright and force `.unwrap_or(...)` everywhere: `.unwrap_or` already exists,
so `??` becomes *pure* sugar with zero new semantics once it targets `Option<T>` — a defensible
"two spellings, one obviously sugar for the other" case, the same category the audit explicitly
waved through for implicit/explicit interface upcasts (Part 1, "intentional flexibility"). This is
different from the `null`-vs-`None` case above because `??`/`.unwrap_or` are an operator/method
pair, not two ways to construct the same *value*.

Mechanically, this changes `??`'s lowering (`src/mir/lower/expr.rs`) from "type-check against
`Type::Nullable`, lower to a MIR comparison against `Const::Null`" to "type-check the LHS as
`Option<T>`, lower to the same call-`unwrap_or` MIR shape the method call itself already produces."
No new MIR node; the analyzer just picks a different desugaring target.

## 3. How is `Option<StructType>` boxing cost handled?

**Decision: accept the cost. `Option<T>` keeps its one existing representation (heap tag +
payload, discriminated union) for every `T`, including value structs. No nullable-style
null-pointer fast path is re-derived for `Option`.**

The alternative — re-deriving `is_nullable_boxed_value`'s null-pointer-as-boxed-value trick for
`Option<StructType>` — would mean maintaining *two* representations for the same generic type
(`Option<ClassType>` as a discriminated union, `Option<StructType>` as a null-checkable boxed
pointer), which is precisely the kind of hidden bifurcation this purge is trying to eliminate from
the type system, just moved one level down (from surface syntax into `Option<T>`'s own codegen).
It would also expand scope substantially: the boxing logic lives in `hir_emit/stmts.rs`,
`emitter/value_struct.rs`, and `release.rs`, all keyed on `TyKind::Nullable` today, and would need
a parallel `TyKind::Option`-aware path added rather than deleted.

Concrete implication: `Option<Node>` where `Node` is a `class` (the common linked-list case flagged
in the audit) costs the same as `Node?` did — a class instance is already a heap pointer, so the
union's tag+pointer payload is no more expensive than a nullable pointer was. The cost increase is
isolated to `Option<S>` where `S` is a `struct` (value type): what was a zero-allocation nullable
value (a null-checkable inline slot) becomes a heap-allocated union, matching how every other
`Option<T>` already behaves. This is a real, deliberate regression for that one case, scoped
narrowly, and it is the trade-off the audit already called out as the one to make explicitly rather
than solve.

This is flagged as revisitable: if `Option<ValueStruct>` boxing cost shows up as a real hot path
later, a targeted "small `Option<T>` inline representation for value-type `T`" optimization pass is
a self-contained follow-up (an MIR/codegen change local to `Option`'s own lowering), not something
that needs to reopen this purge.

## Net effect on migration

With these three decisions fixed, `removal-nullable-implementation` reduces to:

1. Delete the `null` token/literal and `TyKind::Nullable` plus its interner methods
   (`nullable()`, `strip_nullable()`, `unwrap_nullable()`, `is_nullable_boxed_value()`).
2. Follow every compile error at each `strip_nullable`/`Type::Nullable` call site (~25 files under
   `src/semantics/analyzer/**`, plus MIR/codegen: `src/mir/lower/expr.rs`,
   `emitter/rvalue/casts.rs`, `emitter/value_struct.rs`, `release.rs`, `valuetype.rs`,
   `wasm_types.rs`, `js_marshal.rs`, `js_abi.rs`, and `tooling/dream-lsp/src/index/model.rs`) and
   either delete the nullable-specific branch (if `Option<T>` already handles it structurally) or
   retarget it at `Option<T>`'s existing discriminated-union path.
3. Rewrite the `??` lowering to target `Option<T>.unwrap_or` as described above.
4. Migrate every `T?`/`null` fixture and doc page to `Option<T>`/`None`/`.unwrap_or(...)`.
