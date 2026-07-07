# JS Interop

Dream compiles to WebAssembly, so it runs anywhere WASM does — including the browser and Node. Talking to that host is built on three pieces, each with its own page:

| Piece | What it's for | Docs |
| --- | --- | --- |
| `extern fun` | a typed, fixed-signature function that lives in JS (`Math.max`, your glue code) | this page |
| `js` | a dynamic handle to *any* live JS value, used with native syntax | [The js type](references.md) |
| function values | passing functions across the boundary in either direction | [Callbacks](callbacks.md) |

This page covers `extern` functions.

## Declaring an extern function

An `extern fun` has a signature but no body. The compiler lowers it to a WebAssembly *import* and records it in the auto-generated `*.abi.json`. Call it like any other function:

```dream
extern fun alert(msg: string): void;

fun main(): void {
    alert("Hello from Dream!");
}
```

By default the import comes from the `env` module under the function's own name. Three things cooperate:

- `extern fun` declares the signature on the Dream side.
- `@js("module", "field")` optionally remaps which import module and field it binds to.
- The runtime (`runtime/dream.js`) reads the ABI, marshals values, binds externs to JS globals, and bridges Promises for `extern async fun`.

!!! note "Restrictions"
    Extern functions cannot have a body, cannot be generic, and cannot be combined with `public`.

## Remapping the import name

`@js(module, name)` controls which import module and field the extern binds to:

```dream
@js("dom", "setText")            // binds to importObject["dom"]["setText"]
extern fun set_text(value: string): void;

@js("console")                   // module only -> field defaults to the function name
extern fun log(msg: string): void;
```

## Running it from JavaScript

Compiling a `.dream` file produces three artifacts next to it:

- `*.wat` — human-readable WebAssembly text.
- `*.wasm` — the binary module browsers and Node load.
- `*.abi.json` — an auto-generated description of extern imports and exports. You never edit it; the runtime reads it to marshal values.

The `runtime/dream.js` module loads the `.wasm`, wires the built-in `print`/math functions, and runs `main`. The `run` helper finds the sibling `.abi.json` automatically:

```javascript
import { run } from "./runtime/dream.js";

await run("hello.wasm");   // loads hello.abi.json, binds externs, calls main
```

### Auto-binding to JS globals

For every extern you do not supply explicitly, the runtime resolves it against the JS global scope:

- The default `env` module maps to a bare global — `extern fun alert(...)` binds to `alert`.
- `@js("module", "name")` maps to a property — `@js("console", "log")` binds to `console.log`, `@js("Math", "max")` to `Math.max`.

Built-in browser and Node APIs therefore need no glue. Pass `imports` only for your own functions, keyed by the Dream function name:

```javascript
await run("hello.wasm", {
  imports: {
    square: (n) => n * n,
  },
});
```

If an extern matches no global and you don't provide it, the runtime installs a stub that throws only if actually called — so the module still instantiates. For full control, use `load(source, options)` instead of `run`; it returns the instance without calling `main`.

## Value marshaling

With the ABI loaded, arguments and returns convert between Dream's heap layout and JavaScript:

| Dream type | As argument | As return value |
|------------|-------------|-----------------|
| `int`, `float`, `double` | `number` | `number` |
| `bool` | `boolean` | `boolean` |
| `string` | `string` (decoded UTF-8) | return a `string` |
| `T[]` | `Array` of marshaled elements | (pointer) |
| `object`, classes, `List<T>` | opaque pointer (`number`) | (pointer) |

For reference types, read the underlying data with the instance helpers:

```javascript
mod.readString(ptr);          // length-prefixed UTF-8 string
mod.readArray(ptr, "int");    // -> number[]
mod.readList(ptr, "string");  // List<string> -> string[]
mod.readStruct(ptr, [         // class by field schema (declaration order)
  { name: "x", type: "int" },
  { name: "y", type: "int" },
]);
```

To hand a string back to Dream, the runtime calls the exported `malloc` for you (or call `mod.writeString(str)` directly).

## Beyond fixed signatures

`extern fun` is ideal for known signatures. For open-ended JS values (a DOM node, a `fetch` `Response`, a `RegExp`) you want to read and call natively, use the dynamic [`js`](references.md) type:

```dream
let el = js.global.document.getElementById("app");
el.textContent = "hello";
```

Functions cross the boundary in both directions too — see [Callbacks](callbacks.md).

## Built on interop

Several standard-library features are interop wrappers and serve as worked examples:

- [Regex](../stdlib/regex.md) — a cross-runtime regex class (the `regex` crate natively, `RegExp` on JS hosts).
- [HttpClient](../stdlib/http.md) — a cross-runtime HTTP client over `extern async fun`.
