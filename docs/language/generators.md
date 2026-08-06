# Source generators

Dream has **no runtime reflection**. When you need derives, DSLs, or boilerplate that depends on
types and attributes, you write a **compile-time source generator**: Dream modules the host
discovers and runs during the generate pass (before semantic analysis of the merged program).

Generators see a declaration [`SemanticModel`](../stdlib/codegen.md) of `TypeSymbol` / `Symbol`
values — never raw `DefId`s / `TypeId`s / HIR. They either:

1. **Emit** new Dream source (`emit_extend` / `emit_file`) — e.g. `@json` → `to_json` / `from_json`.
2. **Replace** custom syntax-DSL expressions (`html { ... }`) with ordinary Dream expressions.

## Quick start: register a generator

```dream
@generator_module
module myapp.gen.routes;

@generator("routes")
public fun expand_routes(): void {
    // Discovery stub. The host finds this module via import or dream.toml.
}
```

Pull it into a compilation with a normal import, or list it in `dream.toml` next to your entry file
(search walks upward from the entry file's directory):

```toml
[[generators]]
path = "gen/routes.dream"
```

Attributes:

| Attribute | Where | Meaning |
|-----------|--------|---------|
| `@generator_module` | `module` decl | This file participates in generator discovery |
| `@generator("name")` | function | Named generator entry (discovery stub today) |
| `@syntax_block("intro")` | same function | Claims expression DSL `intro { ... }` |


for derives (`@json`, and any new ones you add to `dream-abi`) must be registered there; generators
query them by name on `TypeSymbol` / `Symbol`.

## Worked example: HTML syntax DSL

HTML is **not** a language builtin. The reference sample lives at
[`sample/generators/html/`](../../sample/generators/html/):

- `html.dream` — `@generator_module` + `@syntax_block("html")` + runtime `Html` helpers
- `app.dream` — uses `html { <div>{title}</div> }`
- `dream.toml` — registers the generator

```bash
cargo run -- run sample/generators/html/app.dream
```

When the host sees a registered `@syntax_block("html")`, it lowers each `html { ... }` site to
nested `Html.el(...)` / string concatenation and `replace`s the expression. Without registration,
`html { }` is an error (“unexpanded syntax block”).

Copy that folder to invent your own introducer (`svg { }`, `sql { }`, …): register
`@syntax_block("…")`, provide any runtime helpers the rewrite emits into, and teach the host how to
lower that introducer (same pattern as the HTML sample).

## Worked example: `@json` derive

`@json` is a Dream generator in `system.json` (`JsonGenerator`). The host:

1. Builds a declaration snapshot of every `@json` class / union (fields, variants, `@property_name`,
   `@json_ignore`, generics).
2. Runs the Dream expander (cached WASM harness).
3. `emit_file`s the resulting `extend Type { to_json / from_json }` source into the program.

You use it with no generator registration — the compiler auto-loads `system.json` when any type
carries `@json`. See [JSON](../stdlib/json.md).

## Writing an `emit_extend` derive (pattern)

End-to-end shape for a custom derive (sketch — host APIs are implemented on the Rust
`GeneratorContext` today; Dream modules document the same surface in `system.codegen`):

```dream
import system.codegen;

// 1. Find types marked with your attribute
//    let types = ctx.types_with("my_attr");
//
// 2. For each type, inspect symbols (never DefIds):
//    for (let t in types) {
//        let fields = t.fields();
//        for (let f in fields) {
//            if (f.has_attribute("skip")) { continue; }
//            let name = f.name();
//            let ty = f.type_name();
//            ...
//        }
//    }
//
// 3. Build Dream source with CodeBuilder
let b = CodeBuilder();
b.line("public fun describe(): string {");
b.line("    return \"ok\";");
b.line("}");
// ctx.emit_extend(type_name, b.to_string());
//
// Or emit a whole synthetic file of extends:
// ctx.emit_file("<my-derive>", "extend Foo {\n" + b.to_string() + "}\n");
```

`CodeBuilder` wraps `StringBuilder` (`line` / `append` / `to_string`). Prefer it over raw `string +`
when assembling multi-line method bodies.

### What symbols expose

Useful queries generators rely on (see [codegen stdlib](../stdlib/codegen.md) for the full list):

- **Types:** `types()` / `types_with("attr")` → `TypeSymbol` (class / struct / enum / DU)
- **Members:** `fields()`, `methods()`, `constructors()`, `variants()`
- **Flags:** `is_async`, `is_ref`, `is_static`, visibility
- **Attributes:** `has_attribute("name")`, `attribute_string("name")` (e.g. `@property_name("id")`)

### Emit vs replace

| Goal | API | Result |
|------|-----|--------|
| Add methods to an existing type | `emit_extend(name, body)` | Parsed as `extend Name { … }` |
| Add several extends / free decls | `emit_file(path, source)` | Parsed as a synthetic Dream file |
| Rewrite `intro { … }` DSL | `replace(node, dream_expr)` | Expression site becomes ordinary Dream |

One declaration emit pass and one syntax replace pass run today (no multi-round fixpoint).

## Pipeline position

```text
parse + imports → prelude merge → attribute validate
  → generate (discover → emit → replace)
  → interface defaults → analyze → MIR → WAT
```

Generated `extend` blocks and replacements are analyzed like hand-written code. Errors in generated
source are reported against the synthetic path (`<json-derive>`, etc.).

## Checklist for a useful generator

1. Decide **emit** (derive) vs **replace** (DSL) vs both.
2. Add any trigger attributes to the **closed** registry in `dream-abi` (unknown `@attrs` error).
3. Mark a module `@generator_module` and a function `@generator("…")` (plus `@syntax_block` if needed).
4. Register via import or `[[generators]]`.
5. Emit only Dream the analyzer already accepts — prefer `CodeBuilder`, keep output deterministic.
6. Add a golden test or a sample under `sample/generators/`.

## See also

- [Syntax DSLs](syntax-dsls.md) — introducer + splice rules
- [`system.codegen`](../stdlib/codegen.md) — `CodeBuilder` and host API shapes
- [JSON](../stdlib/json.md) — shipped `@json` derive
- [`sample/generators/html/`](../../sample/generators/html/) — HTML DSL sample
