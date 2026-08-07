# `system.codegen`

Helpers for compile-time source generators.

```dream
import system.codegen;
```

Syntax DSLs are **not** part of this package — see
[Source generators](../language/generators.md). Beginner sample:
[`sample/generators/quote/`](https://github.com/sps014/Dream/tree/main/sample/generators/quote).
Advanced (markup parser + harness):
[`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html).


## Status

| Piece | Today |
|-------|--------|
| `CodeBuilder` | Shipped — build Dream source strings |
| `GenHost` | Shipped — OK/ERR/LOC stdout markers for harnesses |
| `GenResult` (`system.json`) | Shipped — expand outcome + optional type/field for spans |
| Host `GeneratorContext` | Rust only (`driver/generate`) — `emit_*`, `replace`, `error` |
| User `@generator` Dream bodies | **Registered, not executed yet** (syntax-DSL samples use sibling `harness.dream`) |
| Builtin `@json` | Shipped in the driver (language derive) |
| Syntax-DSL harness runner | Shipped — generic snapshot → harness WASM → `replace` |

When a Dream harness runs (as `@json` does), print `GenHost.err_marker()` then the message, and optionally `GenHost.loc_marker()` + `GenHost.format_loc(type, field)` so the host can attach a source span via `DiagnosticBag`. Failures surface as `CompileError::Generator`.

## `CodeBuilder`

Accumulates Dream source for `emit_extend` / `emit_file` bodies. Construct with `CodeBuilder()`
(4 spaces per indent level) or `CodeBuilder.with_spaces(n)` for `n` spaces per level.

#### `CodeBuilder()` / `CodeBuilder.with_spaces(n)`

Creates an empty builder with default or custom indent width. Use `with_spaces` when generated code must match a project's style guide.

```dream
let b = CodeBuilder();
let tight = CodeBuilder.with_spaces(2);
```

#### `indent()` / `dedent()` / `line(text)` / `append(text)` / `to_string()`

Builds source line by line: `line` adds a newline and applies indent at line start; `append` adds inline text without re-indenting mid-line.

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

Indent is applied only at the start of a line. Mid-line `append` does not re-prefix.

## `GenHost`

#### `ok_marker()` / `err_marker()` / `loc_marker()` / `format_loc(type, field)`

Returns the stdout marker strings the compile host expects from generator harnesses. Print `err_marker()` + message on failure; optionally `loc_marker()` + `format_loc` to point at a type field.

```dream
System.println(GenHost.ok_marker());
System.println(GenHost.err_marker());
System.println(GenHost.loc_marker());
System.println(GenHost.format_loc("User", "name"));
```

## `GenResult` (`system.json`)

Outcome of a generator expand step.

#### `GenResult.success(source)` / `failure(error)` / `failure_at(error, type, field)`

Constructs the outcome object a harness returns: success with generated source, plain failure, or failure tied to a type/field for span attachment.

```dream
import system.json;

let ok = GenResult.success("extend User { }");
let bad = GenResult.failure("unsupported field");
let at = GenResult.failure_at("bad type", "User", "age");
System.println(ok.ok);
System.println(bad.error);
```

Fields: `ok`, `source`, `error`, `error_type`, `error_field`.

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
| `@json` derive | [JSON](json.md) — compiler builtin |
| Quote sample | [`sample/generators/quote/`](https://github.com/sps014/Dream/tree/main/sample/generators/quote) |
| HTML sample | [`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html) |

Full tutorial: [Source generators](../language/generators.md).
