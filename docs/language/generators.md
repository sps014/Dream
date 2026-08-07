# Source generators

Dream has **no runtime reflection**. When you need derives, DSLs, or boilerplate that
depends on types and attributes, write a **compile-time source generator**.

Generators inspect declarations and either:

1. **Emit** new Dream source — for example `@json` adds `to_json` / `from_json`. Prefer this when every use site should keep looking like ordinary Dream and the generated code is methods or helpers.
2. **Replace** custom syntax — for example `quote { … }` or `html { … }` become ordinary Dream expressions. Prefer this when call sites need a domain-specific shape the parser would otherwise reject.

You can write a tiny generator (a few dozen lines) or a complex one (a full markup
compiler). Start with `@json` or the
[`quote`](https://github.com/sps014/Dream/tree/main/sample/generators/quote) sample;
study [`html`](https://github.com/sps014/Dream/tree/main/sample/generators/html) when you
need a real DSL. Helpers live in [`system.codegen`](../stdlib/codegen.md).

## Start here: `@json`

Marks a type for the compiler's built-in JSON derive. Generates `to_json` / `from_json` so you can round-trip without writing converters by hand. You do not register a generator — `system.json` loads automatically when any type carries `@json`.

Use `@json` when the payload is a fixed Dream type. Reach for a custom generator when you need a different wire format, a DSL, or emit logic `@json` does not cover.

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

See [JSON](../stdlib/json.md) for field options (`@property_name`, `@json_ignore`, unions, …).

## Your first custom generator: `quote`

`quote { … }` turns the opaque text inside the braces into a Dream string literal at
compile time. From the app side it looks like ordinary Dream — useful for multi-line
snippets, embed templates, or teaching the expand pipeline without a real parser.

Prefer `quote` as a learning sample or for literal string payloads. Prefer `html` (or your own DSL) when the braced body must be parsed and rewritten into structured Dream expressions.

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

Tells the compiler which Dream file owns the generator. List the generator next to your entry file in `dream.toml` (search walks upward from the entry file's directory), or import a file that contains the `@generator` function.

Use `dream.toml` when the generator should always load with the project; use an import when apps opt in by depending on a module.

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
The host runs sibling `harness.dream` to expand sites. Put expand logic in the harness (or a future executed body), not in this stub.

#### `@generator`

Marks a top-level function as a generator entry. The function name is the generator's identity in registration and diagnostics. Apply it to every custom generator, even when the body is empty.

#### `@syntax_block("introducer")`

Claims expression DSL sites of the form `introducer { … }`. Without it, those sites fail with “unexpanded syntax block”. Pair with `@generator` on the same function when your feature is a replace-style DSL.

| Attribute | Where | Meaning |
|-----------|--------|---------|
| `@generator` | function | Generator entry; name = function name |
| `@syntax_block("intro")` | same function | Claims expression DSL `intro { … }` |

### Generator author: harness

`harness.dream` next to `gen.dream` is the expand worker for syntax DSLs. It reads a snapshot JSON file the host writes, builds a Dream expression for each site, and prints `GenHost` OK lines `id\tdream_expr` so the host can `replace` those sites before type-checking.

Write a harness whenever you use `@syntax_block`. Keep it small for literal rewrites (`quote`); grow it into a compiler when the body needs parsing (`html`).

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

#### `GenHost.ok_marker()` / `err_marker()` / `loc_marker()` / `format_loc(type, field)`

Stdout protocol strings the compile host understands. Print `ok_marker()` then each `id\texpr` line on success. On failure, print `err_marker()` then the message; optionally `loc_marker()` + `format_loc` so diagnostics can point at a type field.

Use these instead of ad-hoc prefixes — the host only recognizes this protocol when applying replace lines or turning failures into `CompileError::Generator`.

See the full harness in the [quote sample](https://github.com/sps014/Dream/tree/main/sample/generators/quote/harness.dream). API details: [CodeBuilder / GenHost](../stdlib/codegen.md).

### How expand works

The host owns discovery and rewrite; your harness owns turning opaque text into Dream source.

1. Registration claims introducer `quote` (via `@syntax_block`).
2. Host snapshots each `quote { }` site and runs sibling `harness.dream`.
3. Harness prints `GenHost` OK lines `id\tdream_expr`.
4. Host replaces those sites with the expressions before type-checking.

Rules for any syntax DSL:

- The introducer is a bare identifier (`quote`, `html`, …) — not a keyword. Pick a name that will not collide with user identifiers in the same scope.
- Inside the braces, non-splice text is opaque to the Dream parser. That is why you need a harness — the main parser does not understand your DSL grammar.
- `{ … }` splices (when your DSL supports them) must be valid Dream expressions; they type-check after rewrite. Use splices to embed runtime values inside generated literals or builders.
- Every introducer must be claimed by a registered `@syntax_block("…")`. Unregistered sites fail with “unexpanded syntax block”.

## Complex example: HTML

Same call-site shape as `quote`, but the sample adds a markup parser and runtime helpers.
Use this pattern when the braced body is a real language (tags, attributes, nested structure) that should lower to ordinary Dream expressions — not a raw string.

User-facing code stays readable; complexity lives in the sample's `HtmlCompiler` + harness.

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

Custom attributes annotate declarations so generators can find and configure them. Use them for routes, columns, permissions, or any metadata that should drive emit/replace without hard-coding type names in the generator.

#### Declaring with `@attribute`

Define the attribute schema with `@attribute` on a bare top-level function. The function name is the attribute name; its parameters are the `@name(...)` argument schema. Empty bodies are fine — this is a schema declaration, not runtime code.

```dream
@attribute
public fun route(path: string): void { }
```

#### Applying the attribute

Attach the attribute to the declarations your generator should see:

```dream
@route("/users")
public fun list_users(): void { }
```

#### Querying from a generator

Host discovery APIs (Rust `GeneratorContext` today; see [codegen](../stdlib/codegen.md)):

- `functions_with("route")` / `types_with("attr")` — find annotated symbols.
- `attribute_args("route")` / `attribute_string("name")` / `has_attribute("name")` — read arguments.

Trigger attributes such as `@json` must be known to the language. Generators query attributes by name on declaration symbols — they do not re-parse source for `@…` text.

## Emitting source with CodeBuilder

Builds indented Dream source strings for `emit_extend` / `emit_file`. Prefer `CodeBuilder` over manual `"\n" + "    "` concatenation when generating multi-line methods — indent stays consistent and easier to edit.

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

#### Host emit / replace APIs

| Goal | API | When to use it |
|------|-----|----------------|
| Add methods to an existing type | `emit_extend(name, body)` | Derive-style generators (`to_json`, helpers) that attach to user types |
| Emit several extends or free declarations | `emit_file(path, source)` | Larger synthetic modules, multiple types, or shared helpers |
| Rewrite `intro { … }` | `replace(node, dream_expr)` | Syntax DSLs — turns a site into an ordinary expression before type-check |
| Report a generate-time error | `ctx.error(node, message)` | Validation failures that should become `CompileError::Generator` |

#### Useful queries on declarations

Use these to decide what to emit without hard-coding every type name:

- `types()` / `types_with("attr")` / `functions_with("attr")` — enumerate work items.
- `fields()` / `methods()` / `constructors()` / `variants()` — walk members for derives.
- `has_attribute("name")` / `attribute_string("name")` / `attribute_args("name")` — read configuration.
- `is_async` / `is_ref` / `is_static` — specialize generated signatures.

Full reference: [CodeBuilder](../stdlib/codegen.md).

## Checklist

1. Decide **emit** (derive) vs **replace** (DSL) vs both — emit keeps call sites ordinary; replace invents new syntax.
2. Use builtin attributes, or define your own with `@attribute` on a top-level function.
3. Mark a function `@generator` (plus `@syntax_block` if you claim an introducer).
4. For a syntax DSL: ship sibling `harness.dream` that reads the host snapshot and prints replace lines via `GenHost`.
5. Register via import or `[[generators]]` in `dream.toml`.
6. Prefer `CodeBuilder` for multi-line bodies.
7. Report failures via harness OK/ERR markers (or host `ctx.error`) so they become `CompileError::Generator`.
8. Add a sample under `sample/generators/` or a golden test.

## See also

- [CodeBuilder](../stdlib/codegen.md) — `CodeBuilder`, `GenHost`, `GenResult`
- [JSON](../stdlib/json.md) — builtin `@json` derive
- Beginner sample: [`sample/generators/quote/`](https://github.com/sps014/Dream/tree/main/sample/generators/quote)
- Advanced sample: [`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html)
