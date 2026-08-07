# Source generators

Dream has **no runtime reflection**. When you need derives, DSLs, or boilerplate that
depends on types and attributes, write a **compile-time source generator**.

Generators inspect declarations and either:

1. **Emit** new Dream source — for example `@json` adds `to_json` / `from_json`.
2. **Replace** custom syntax — for example `quote { … }` or `html { … }` become ordinary
   Dream expressions.

You can write a tiny generator (a few dozen lines) or a complex one (a full markup
compiler). Start with `@json` or the
[`quote`](https://github.com/sps014/Dream/tree/main/sample/generators/quote) sample;
study [`html`](https://github.com/sps014/Dream/tree/main/sample/generators/html) when you
need a real DSL.

## Start here: `@json`

Mark a type `@json` and the compiler generates `to_json` / `from_json`. You do not register
a generator — `system.json` loads automatically when any type carries `@json`.

```dream
import system;
import system.json;

@json
class Point {
    public x: int;
    public y: int;

    public constructor(x: int, y: int) {
        this.x = x;
        this.y = y;
    }
}

fun main(): void {
    let p = Point(1, 2);
    let text = Json.serialize(p);
    let back = Json.deserialize<Point>(text).unwrap_or(p);
    System.println(back.x);
}
```

See [JSON](../stdlib/json.md).

## Your first custom generator: `quote`

`quote { … }` turns the opaque text inside the braces into a Dream string literal at
compile time. From the app side it looks like ordinary Dream:

```dream
import system;

fun main() {
    System.println(quote { Hello generators });
}
```

```bash
cargo run -- run sample/generators/quote/app.dream
```

Expected stdout: `Hello generators`

Full sample: [`sample/generators/quote/`](https://github.com/sps014/Dream/tree/main/sample/generators/quote).

### Register it

List the generator next to your entry file in `dream.toml` (search walks upward from the
entry file's directory), or import a file that contains the `@generator` function:

```toml
[[generators]]
path = "gen.dream"
```

```dream
module gen;

import system.codegen;

@generator
@syntax_block("quote")
public fun quote(): void { }
```

The empty body is intentional: user `@generator` functions are **registered, not executed**.
The host runs sibling `harness.dream` to expand sites.

| Attribute | Where | Meaning |
|-----------|--------|---------|
| `@generator` | function | Generator entry; name = function name |
| `@syntax_block("intro")` | same function | Claims expression DSL `intro { … }` |

### Generator author: harness

`harness.dream` next to `gen.dream` reads a snapshot JSON file, builds a Dream expression
for each site, and prints `GenHost` OK lines `id\tdream_expr`:

```dream
import system;
import system.io;
import system.collections;
import system.json;
import system.codegen;

fun as_dream_string(s: string): string {
    let escaped = s.replace("\\", "\\\\").replace("\"", "\\\"");
    return "\"" + escaped + "\"";
}

async fun main(): void {
    let path = System.env_or("DREAM_SYNTAX_GEN_SNAPSHOT", "");
    // … read path, Json.parse …
    // for each block: out_lines.push(id + "\t" + as_dream_string(body.trim()));
    System.println(GenHost.ok_marker());
    // … println each out line …
}
```

See the full harness in the [quote sample](https://github.com/sps014/Dream/tree/main/sample/generators/quote/harness.dream).

### How expand works

1. Registration claims introducer `quote` (via `@syntax_block`).
2. Host snapshots each `quote { }` site and runs sibling `harness.dream`.
3. Harness prints `GenHost` OK lines `id\tdream_expr`.
4. Host replaces those sites with the expressions before type-checking.

Rules for any syntax DSL:

- The introducer is a bare identifier (`quote`, `html`, …) — not a keyword.
- Inside the braces, non-splice text is opaque to the Dream parser.
- `{ … }` splices (when your DSL supports them) must be valid Dream expressions; they
  type-check after rewrite.
- Every introducer must be claimed by a registered `@syntax_block("…")`. Unregistered
  sites fail with “unexpanded syntax block”.

## Complex example: HTML

Same call-site shape as `quote`, but the sample adds a markup parser and runtime helpers.
User-facing code:

```dream
import system;
import html;

fun page(title: string): string {
    return html {
        <div class="hero">
            <h1>{title}</h1>
            <p>Welcome</p>
        </div>
    };
}

fun main() {
    System.println(page("Hello"));
}
```

```bash
cargo run -- run sample/generators/html/app.dream
```

HTML is **not** a language builtin. Expand is owned by
[`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html)
(Dream `HtmlCompiler` + `harness.dream`, registered via `@syntax_block("html")`). The host
only snapshots sites and applies replace lines — no Rust markup parser. Protocol is the
same as `quote`; the harness is larger because it parses tags and `{expr}` splices.

## User-defined attributes

Use a custom attribute on your declarations (generators query them later):

```dream
@route("/users")
public fun list_users(): void { }
```

Define the attribute schema with `@attribute` on a bare top-level function. The function
name is the attribute name; its parameters are the `@name(...)` argument schema:

```dream
@attribute
public fun route(path: string): void { }
```

Generators query with `functions_with("route")` / `attribute_args("route")`.

Trigger attributes such as `@json` must be known to the language. Generators query
attributes by name on declaration symbols.

## Emitting source with CodeBuilder

```dream
import system.codegen;

let b = CodeBuilder();
b.line("public fun describe(): string {");
b.indent();
b.line("return \"ok\";");
b.dedent();
b.line("}");
let body = b.to_string();
// Host APIs: emit_extend(type_name, body) or emit_file(path, source)
```

| Goal | API | Result |
|------|-----|--------|
| Add methods to an existing type | `emit_extend(name, body)` | `extend Name { … }` |
| Emit several extends | `emit_file(path, source)` | Synthetic Dream file |
| Rewrite `intro { … }` | `replace(node, dream_expr)` | Ordinary expression |

Useful queries on declarations (see [CodeBuilder](../stdlib/codegen.md)):

- `types()` / `types_with("attr")` / `functions_with("attr")`
- `fields()` / `methods()` / `constructors()` / `variants()`
- `has_attribute("name")` / `attribute_string("name")` / `attribute_args("name")`
- `is_async` / `is_ref` / `is_static`

## Checklist

1. Decide **emit** (derive) vs **replace** (DSL) vs both.
2. Use builtin attributes, or define your own with `@attribute` on a top-level function.
3. Mark a function `@generator` (plus `@syntax_block` if needed).
4. For a syntax DSL: ship sibling `harness.dream` that reads the host snapshot and prints replace lines.
5. Register via import or `[[generators]]`.
6. Prefer `CodeBuilder` for multi-line bodies.
7. Report failures via harness OK/ERR markers (or host `ctx.error`) so they become `CompileError::Generator`.
8. Add a sample under `sample/generators/` or a golden test.

## See also

- [CodeBuilder](../stdlib/codegen.md)
- [JSON](../stdlib/json.md) (`@json` derive)
- Beginner sample: [`sample/generators/quote/`](https://github.com/sps014/Dream/tree/main/sample/generators/quote)
- Advanced sample: [`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html)
