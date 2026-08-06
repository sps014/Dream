# Syntax DSLs

A **syntax DSL** is an expression of the form `introducer { … }` that a source generator rewrites
into ordinary Dream before analysis. Markup or other domain text lives in the braces; `{ expr }`
splices are real Dream expressions captured by the parser and handed to the generator.

```dream
return html {
    <div class="hero">
        <h1>{title}</h1>
    </div>
};
```

## Rules

- The introducer is a bare identifier (`html`, `svg`, …) — not a keyword.
- Inside the braces, non-splice text is opaque to the Dream parser (tokenized as DSL body text).
- `{ … }` splices must be valid Dream expressions; they type-check in the surrounding function after
  rewrite.
- Every introducer must be claimed by a registered `@syntax_block("…")` generator. Unregistered
  sites fail with “unexpanded syntax block”.

## Registration

```dream
@generator_module
module html;

@generator("html")
@syntax_block("html")
public fun expand_html(): void { }
```

Import that module or list it under `[[generators]]` in `dream.toml`. See
[Source generators](generators.md).

## Reference sample

[`sample/generators/html/`](../../sample/generators/html/) is the worked HTML example: registration,
runtime `Html` helpers, and a tiny `app.dream`. Run:

```bash
cargo run -- run sample/generators/html/app.dream
```

HTML is **not** a builtin language feature — the sample owns the helpers; the host expands `html { }`
only when that generator is registered.
