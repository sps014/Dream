# Quote sample

Tiny compile-time `quote { … }` syntax DSL: opaque text in the braces becomes a Dream string literal.

## User-facing code

```dream
import system;

fun main() {
    System.println(quote { Hello generators });
}
```

## Run

```bash
# from repo root
cargo run -- run sample/generators/quote/app.dream
```

Expected stdout:

```text
Hello generators
```

## Layout

| File | Role |
|------|------|
| `app.dream` | Program that uses `quote { … }` |
| `gen.dream` | `@generator` + `@syntax_block("quote")` |
| `harness.dream` | Snapshot in → string-literal replace lines out |
| `dream.toml` | `[[generators]] path = "gen.dream"` |

Start here before the larger [`../html/`](../html/) sample. See [Source generators](../../../docs/language/generators.md).
