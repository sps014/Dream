# HTML sample

Compile-time `html { <tags>… }` syntax DSL owned entirely by this sample (Dream parser + harness).
The compiler host only runs `harness.dream` and applies `replace` — it does not parse markup.

## Layout

| File | Role |
|------|------|
| `html.dream` | Runtime `Html.el` / `render` / `text` |
| `parser.dream` | `HtmlCompiler` — markup → Dream `Html.el` source |
| `gen.dream` | `@generator` + `@syntax_block("html")` |
| `harness.dream` | WASM entry: snapshot in → replace lines out |
| `app.dream` | Tiny program using `html { … }` |
| `dream.toml` | `[[generators]] path = "gen.dream"` |

## Run

```bash
# from repo root
cargo run -- run sample/generators/html/app.dream
```

Expected stdout:

```text
<div class="hero"><h1>Hello</h1><p>Welcome</p></div>
```

## How expand works

1. Registration claims introducer `html`.
2. Host snapshots each `html { }` site (`body` + splice sources) and runs sibling `harness.dream`.
3. Harness calls `HtmlCompiler.compile` and prints `GenHost` OK lines `id\tdream_expr`.
4. Host `ctx.replace`s those expressions before type-checking.

See [Source generators](../../docs/language/generators.md).
