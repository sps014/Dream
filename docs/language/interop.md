# JS Interop

Dream compiles to WebAssembly, so it runs anywhere WASM does, including the browser and Node.
Talking to that host — calling its functions, reading its values, handing it callbacks — is built
on three pieces, each covered by its own page:

| Piece | What it's for | Docs |
| --- | --- | --- |
| `extern fun` | a typed, fixed-signature function that lives in JS (`Math.max`, your own glue code) | this page |
| `js` | a dynamic handle to *any* live JS value, read/called with native syntax | [The js type](references.md) |
| function values | passing functions across the boundary in either direction | [Callbacks](callbacks.md) |

This page covers the first: declaring, binding, and running `extern` functions.

## Declaring an extern function

An `extern fun` has a signature but no body. The compiler lowers it to a WebAssembly *import* and
records it in the auto-generated `*.abi.json`. You call it like any other function:

```dream
extern fun alert(msg: string): void;

fun main(): void {
    alert("Hello from Dream!");
}
```

By default the import comes from the `env` module under the function's own name.

There is one interop mechanism, made of three pieces:

- `extern fun` declares the function on the Dream side.
- `@js("module", "field")` is an optional attribute that remaps which import module and field the
  function binds to.
- The runtime (`runtime/dream.js`) reads the ABI, marshals values across the boundary, binds
  externs to JS globals, and bridges Promises for `extern async fun`.

## Remapping the import name

Use the `@js(module, name)` attribute to control which import module and field the extern binds to:

```dream
// binds to importObject["dom"]["setText"]
@js("dom", "setText")
extern fun set_text(value: string): void;

// only the module given -> field defaults to the function name
@js("console")
extern fun log(msg: string): void;
```

!!! note "Restrictions"
    Extern functions cannot have a body, cannot be generic, and cannot be combined with `public`.

## Running it from JavaScript

Compiling a `.dream` file automatically produces three artifacts next to it:

- `*.wat` — the human-readable WebAssembly text.
- `*.wasm` — the binary module browsers and Node load.
- `*.abi.json` — an auto-generated description of the extern imports and exports. You never write or edit this; the runtime reads it to marshal values for you.

The `runtime/dream.js` ES module loads the `.wasm`, wires the built-in `print`/math functions, and runs `main`. The `run` helper derives the sibling `.abi.json` automatically, so a whole page can be one call:

```javascript
import { run } from "./runtime/dream.js";

await run("hello.wasm");   // loads hello.abi.json, binds externs, calls main
```

### Auto-binding to JS globals

For every extern you do not supply explicitly, the runtime resolves it against the JavaScript global
scope:

- The default `env` module maps to a bare global: `extern fun alert(...)` binds to `alert`.
- `@js("module", "name")` maps to a property of that global: `@js("console", "log")` binds to
  `console.log`, `@js("Math", "max")` to `Math.max`.

Built-in browser and Node APIs therefore need no glue. Pass `imports` only for your own functions:

```javascript
await run("hello.wasm", {
  imports: {
    square: (n) => n * n,   // keyed by the Dream function name
  },
});
```

If an extern matches no global and you do not provide it, the runtime installs a stub that throws only if it is actually called — so the module still instantiates.

When you need full control, use `load(source, options)` instead of `run`; it returns the instance without calling `main`.

## Value marshaling

With the ABI loaded, arguments and return values are converted between Dream's heap layout and JavaScript:

| Dream type | JavaScript value (as argument) | As return value |
|--------------|-------------------------------|-----------------|
| `int`, `float`, `double` | `number` | `number` |
| `bool` | `boolean` | `boolean` |
| `string` | `string` (decoded UTF-8) | return a `string` |
| `T[]` | `Array` of marshaled elements | (pointer) |
| `object`, classes, `List<T>` | opaque pointer (`number`) | (pointer) |

For reference types you can read the underlying data with the instance helpers:

```javascript
mod.readString(ptr);          // length-prefixed UTF-8 string
mod.readArray(ptr, "int");    // -> number[]
mod.readList(ptr, "string");  // List<string> -> string[]
mod.readStruct(ptr, [         // class by field schema (declaration order)
  { name: "x", type: "int" },
  { name: "y", type: "int" },
]);
```

To hand a string back to Dream from a JS implementation, the runtime calls the exported `malloc` for you (or you can call `mod.writeString(str)` directly).

## Dynamic JavaScript values

`extern fun` is great for a fixed, known signature, but a lot of real JS APIs hand back open-ended
values — a DOM node, a `fetch` `Response`, a `RegExp` — that you want to read and call natively
rather than flatten to a string. That's what the dynamic [`js`](references.md) type is for:

```dream
let el = js.global.document.getElementById("app");
el.textContent = "hello";
```

Head to [The js type](references.md) for the full API: getting values, calling methods, passing
structs/classes, and awaiting Promises.

## Callbacks

Functions cross the boundary in both directions: pass a Dream `fun(...)` to JavaScript, or hand a JS
function into Dream and call it directly. See [Callbacks](callbacks.md) for both directions,
including registering DOM event handlers.

## Built on interop

Several standard-library features are interop wrappers built on the pieces above, and serve as
worked examples:

- [Regex](../stdlib/regex.md) — a cross-runtime regular-expression class (the `regex` crate
  natively, `RegExp` on JS hosts).
- [HttpClient](../stdlib/http.md) — a cross-runtime HTTP client over `extern async fun`.
