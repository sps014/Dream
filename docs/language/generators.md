# Source generators

Dream has **no runtime reflection**. When you need derives, DSLs, or boilerplate that depends on
types and attributes, write a **compile-time source generator**.

Generators inspect declarations and either:

1. **Emit** new Dream source — for example `@json` adds `to_json` / `from_json`.
2. **Replace** custom syntax — for example `html { … }` becomes ordinary Dream expressions.

## Register a generator

```dream
import system.codegen;

@generator
public fun routes(): void {
    // Discovery stub — name is the function name (`routes`).
    // Declaration bodies are not executed yet; syntax-DSL samples ship a sibling `harness.dream`
    // the host runs (same idea as the `@json` harness). Use CodeBuilder / GenHost in harnesses.
}
```

Pull it into a compilation with a normal import, or list it in `dream.toml` next to your entry
file (search walks upward from the entry file's directory):

```toml
[[generators]]
path = "gen/routes.dream"
```

| Attribute | Where | Meaning |
|-----------|--------|---------|
| `@generator` | function | This function is a generator entry; name = function name |
| `@syntax_block("intro")` | same function | Claims expression DSL `intro { … }` |

### User-defined attributes

Mark a bare top-level function `@attribute`. The function name is the attribute name (exact
casing); its parameters are the `@name(...)` argument schema:

```dream
@attribute
public fun route(path: string): void { }

@route("/users")
public fun list_users(): void { }
```

Generators query with `functions_with("route")` / `attribute_args("route")`.

Trigger attributes such as `@json` must be known to the language. Generators query attributes by
name on declaration symbols.

## Syntax DSLs

A **syntax DSL** is an expression `introducer { … }` that a generator rewrites into ordinary Dream
before type-checking. Markup or other domain text lives in the braces; `{ expr }` splices are real
Dream expressions.

```dream
return html {
    <div class="hero">
        <h1>{title}</h1>
    </div>
};
```

Rules:

- The introducer is a bare identifier (`html`, `svg`, …) — not a keyword.
- Inside the braces, non-splice text is opaque to the Dream parser.
- `{ … }` splices must be valid Dream expressions; they type-check after rewrite.
- Every introducer must be claimed by a registered `@syntax_block("…")`. Unregistered sites fail
  with “unexpanded syntax block”.

### HTML sample

HTML is **not** a language builtin. Expand is owned by
[`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html):
Dream `HtmlCompiler` + `harness.dream`, registered via `@syntax_block("html")`. The host only
snapshots sites and applies replace lines (no Rust markup parser).

```bash
cargo run -- run sample/generators/html/app.dream
```

Convention for your own introducer: register `@generator` + `@syntax_block("…")`, put a
`harness.dream` next to the generator file, lower sites in Dream, print `GenHost` OK/ERR lines.

## `@json` derive

Mark a type `@json` and the compiler generates `to_json` / `from_json`. You do not register a
generator yourself — `system.json` is loaded automatically when any type carries `@json`.

See [JSON](../stdlib/json.md).

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
- [Attributes](attributes.md) (if present) / language overview
