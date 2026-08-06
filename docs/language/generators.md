# Source generators

Dream has **no runtime reflection**. When you need derives, DSLs, or boilerplate that depends on
types and attributes, write a **compile-time source generator**.

Generators inspect declarations and either:

1. **Emit** new Dream source — for example `@json` adds `to_json` / `from_json`.
2. **Replace** custom syntax — for example `html { … }` becomes ordinary Dream expressions.

## Register a generator

```dream
@generator_module
module myapp.gen.routes;

@generator("routes")
public fun expand_routes(): void {
    // Discovery stub — the compiler finds this module via import or dream.toml.
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
| `@generator_module` | `module` decl | This file participates in generator discovery |
| `@generator("name")` | function | Named generator entry |
| `@syntax_block("intro")` | same function | Claims expression DSL `intro { … }` |

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

HTML is not a language builtin. The reference sample is
[`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html):

```bash
cargo run -- run sample/generators/html/app.dream
```

That folder registers `@syntax_block("html")`, provides runtime `Html` helpers, and shows a tiny
app. Copy it to invent your own introducer (`svg { }`, `sql { }`, …).

## `@json` derive

Mark a type `@json` and the compiler generates `to_json` / `from_json`. You do not register a
generator yourself — `system.json` is loaded automatically when any type carries `@json`.

See [JSON](../stdlib/json.md).

## Emitting source with CodeBuilder

```dream
import system.codegen;

let b = CodeBuilder();
b.line("public fun describe(): string {");
b.line("    return \"ok\";");
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

- `types()` / `types_with("attr")`
- `fields()` / `methods()` / `constructors()` / `variants()`
- `has_attribute("name")` / `attribute_string("name")`
- `is_async` / `is_ref` / `is_static`

## Checklist

1. Decide **emit** (derive) vs **replace** (DSL) vs both.
2. Use only attributes the language already recognizes (or get them added).
3. Mark a module `@generator_module` and a function `@generator("…")` (plus `@syntax_block` if needed).
4. Register via import or `[[generators]]`.
5. Prefer `CodeBuilder` for multi-line bodies.
6. Add a sample under `sample/generators/` or a golden test.

## See also

- [CodeBuilder](../stdlib/codegen.md)
- [JSON](../stdlib/json.md)
- [`sample/generators/html/`](https://github.com/sps014/Dream/tree/main/sample/generators/html)
