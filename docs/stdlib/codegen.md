# `system.codegen`

Helpers for compile-time source generators.

```dream
import system.codegen;
```

HTML and other syntax DSLs are **not** part of this package — see
[`sample/generators/html/`](../../sample/generators/html/) and [Syntax DSLs](../language/syntax-dsls.md).

## `CodeBuilder`

Accumulates Dream source for `emit_extend` / `emit_file` bodies. Backed by `StringBuilder` (from
`system.text`); construct with `CodeBuilder()`, not a static factory.

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

## Generator host API (documented)

The compile host (`driver/generate`) exposes this shape to generators. Dream modules discover work
via `@generator_module` / attributes; the host fills `SemanticModel` / `SyntaxTree` and applies
emits:

### Discovery

- `ctx.types()` / `ctx.types_with("attr")` → `TypeSymbol[]`
- `ctx.functions_with("attr")` → `Symbol[]`
- `ctx.syntax_blocks("introducer")` → syntax node ids for `introducer { … }`

### Symbols

`TypeSymbol` / `Symbol` cover class, struct, ref struct, enum, discriminated union, fields, methods,
constructors, variants, async, ref, weak/unowned, and attributes:

- `has_attribute("name")` / `attribute_string("name")`
- `fields()` / `methods()` / `constructors()` / `variants()`
- `is_async` / `is_ref` / `is_static` / visibility

Symbols are the only identity generators see — not `DefId` / `TypeId`.

### Emit / replace

- `ctx.emit_extend(type_name, body)` — synthesize `extend Type { body }`
- `ctx.emit_file(synthetic_path, source)` — parse a whole synthetic Dream file of extends
- `ctx.replace(node, dream_expr)` — rewrite a syntax-DSL site to an expression
- `ctx.error(node, message)` — report a generate-time diagnostic

### Shipped generators

| Feature | Where |
|---------|--------|
| `@json` derive | Dream `JsonGenerator` in `system.json` (host snapshot + harness) |
| `html { }` DSL | [`sample/generators/html/`](../../sample/generators/html/) (register `@syntax_block`) |

Full tutorial: [Source generators](../language/generators.md).
