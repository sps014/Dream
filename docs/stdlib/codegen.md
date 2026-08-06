# `system.codegen`

Helpers for compile-time source generators.

```dream
import system.codegen;
```

HTML and other syntax DSLs are **not** part of this package — see
[Source generators](../language/generators.md) and
[`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html).

## `CodeBuilder`

Accumulates Dream source for `emit_extend` / `emit_file` bodies. Construct with `CodeBuilder()`.

```dream
let b = CodeBuilder();
b.line("public fun to_json(): JsonValue {");
b.line("    return JsonValue.dict();");
b.line("}");
b.append("// trailing comment");
let src = b.to_string();
```

| Method | Role |
|--------|------|
| `CodeBuilder()` | Empty builder |
| `line(text)` | Append a line (adds a trailing newline) |
| `append(text)` | Append raw text with no extra newline |
| `to_string()` | Materialize the buffer |

## Generator host API

Generators discover work via `@generator_module` / attributes. The compile host exposes:

### Discovery

- `ctx.types()` / `ctx.types_with("attr")` → type symbols
- `ctx.functions_with("attr")` → function symbols
- `ctx.syntax_blocks("introducer")` → sites for `introducer { … }`

### Symbols

Type and member symbols cover class, struct, enum, union, fields, methods, constructors, variants,
async, ref, and attributes:

- `has_attribute("name")` / `attribute_string("name")`
- `fields()` / `methods()` / `constructors()` / `variants()`
- `is_async` / `is_ref` / `is_static` / visibility

### Emit / replace

- `ctx.emit_extend(type_name, body)` — synthesize `extend Type { body }`
- `ctx.emit_file(path, source)` — parse a synthetic Dream file
- `ctx.replace(node, dream_expr)` — rewrite a syntax-DSL site
- `ctx.error(node, message)` — report a generate-time diagnostic

### Shipped generators

| Feature | Where |
|---------|--------|
| `@json` derive | [JSON](json.md) |
| `html { }` DSL | [`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html) |

Full tutorial: [Source generators](../language/generators.md).
