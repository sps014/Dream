# HTML syntax-DSL generator sample

This folder is a **worked example** of a Dream source generator that owns a custom expression
DSL: `html { ... }`.

There is no builtin HTML language feature. The compiler expands `html { }` only when a generator
registers `@syntax_block("html")` (this sample does that in `html.dream`).

## Layout

| File | Role |
|------|------|
| `html.dream` | `@generator_module` + `@syntax_block("html")` + runtime `Html` helpers |
| `app.dream` | Tiny program that uses the DSL |
| `dream.toml` | `[[generators]]` so you need not import the gen module |

## Run

```bash
# from repo root
cargo run -- run sample/generators/html/app.dream
```

Expected stdout:

```text
<div class="hero"><h1>Hello</h1><p>Welcome</p></div>
```

## How it wires up

1. `dream.toml` registers `html.dream` (or `import html;`).
2. During the generate pass the host finds every `html { ... }` expression, lowers markup +
   `{expr}` splices to nested `Html.el(...)` / string concatenation, and `replace`s the site.
3. Ordinary analysis/codegen then type-checks the rewritten Dream like any other expression.

## Writing your own DSL

1. Copy this folder; rename the introducer (`@syntax_block("svg")`, …).
2. Keep runtime helpers in the same generator module (or a sibling import).
3. Register via `dream.toml` or import.
4. For derive-style generators (emit methods on types), use `CodeBuilder` + `emit_extend` as in
   [Source generators](../../docs/language/generators.md) — `@json` is the shipped derive sample.
