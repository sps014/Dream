# `system.codegen`

Helpers for compile-time source generators.

```dream
import system.codegen;
```

HTML and other syntax DSLs are **not** part of this package — see
[Source generators](../language/generators.md) and
[`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html).

## Status

| Piece | Today |
|-------|--------|
| `CodeBuilder` | Shipped — build Dream source strings |
| `GenHost` | Shipped — OK/ERR/LOC stdout markers for harnesses |
| `GenResult` (`system.json`) | Shipped — expand outcome + optional type/field for spans |
| Host `GeneratorContext` | Rust only (`driver/generate`) — `emit_*`, `replace`, `error` |
| User `@generator` Dream bodies | **Registered, not executed yet** — use `@json` / `html` as patterns |

When a Dream harness runs (as `@json` does), print `GenHost.err_marker()` then the message, and optionally `GenHost.loc_marker()` + `GenHost.format_loc(type, field)` so the host can attach a source span via `DiagnosticBag`. Failures surface as `CompileError::Generator`.

## `CodeBuilder`

Accumulates Dream source for `emit_extend` / `emit_file` bodies. Construct with `CodeBuilder()`
(4 spaces per indent level) or `CodeBuilder.with_spaces(n)` for `n` spaces per level.

```dream
let b = CodeBuilder();
b.line("public fun to_json(): JsonValue {");
b.indent();
b.line("return JsonValue.dict();");
b.dedent();
b.line("}");
b.append("// trailing comment");
let src = b.to_string();
```

| Method | Role |
|--------|------|
| `CodeBuilder()` | Empty builder, 4 spaces per indent level |
| `CodeBuilder.with_spaces(n)` | Empty builder, `n` spaces per level (`<= 0` → no indent) |
| `indent()` | Increase indent level by one |
| `dedent()` | Decrease indent level by one (floored at 0) |
| `line(text)` | Write indent (if at line start) + text + newline |
| `append(text)` | Write indent (if at line start) + text (no extra newline) |
| `to_string()` | Materialize the buffer |

Indent is applied only at the start of a line (after `line`, or initially). Mid-line `append`
does not re-prefix.

## `GenHost`

| Method | Role |
|--------|------|
| `ok_marker()` / `err_marker()` / `loc_marker()` | Stdout protocol lines for harnesses |
| `format_loc(type, field)` | `type\tfield` span hint after `loc_marker` |

## Generator host API (Rust)

Generators discover work via `@generator` functions / attributes. The compile host exposes:

### Discovery

- `ctx.types()` / `ctx.types_with("attr")` → type symbols
- `ctx.functions_with("attr")` → function symbols
- `ctx.syntax_blocks("introducer")` → sites for `introducer { … }`

### Emit / replace / errors

- `ctx.emit_extend(type_name, body)` — synthesize `extend Type { body }`
- `ctx.emit_file(path, source)` — parse a synthetic Dream file
- `ctx.replace(node, dream_expr)` — rewrite a syntax-DSL site
- `ctx.error(node, message)` — queue a generate-time diagnostic (flushed into `DiagnosticBag`)

### Shipped generators

| Feature | Where |
|---------|--------|
| `@json` derive | [JSON](json.md) |
| `html { }` DSL | [`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html) |

Full tutorial: [Source generators](../language/generators.md).
