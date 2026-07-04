# Dream JS interop samples

Each `*.dream` here compiles to WebAssembly and talks to JavaScript through the shared runtime in
[`runtime/dream.js`](../../runtime/dream.js) — automatic value marshaling for strings, arrays,
`List<T>`, structs, callbacks, the dynamic `js` type, `fetch`, and `extern async`.

## Build the artifacts first

The compiled `*.wasm`, `*.wat`, and `*.abi.json` files are **git-ignored**, so a fresh checkout has
none. Build the one you want before running it:

```sh
# from the repository root
cargo run -- sample/interop/interop.dream      # writes interop.wasm + interop.abi.json (+ .wat)
```

Or build them all at once:

```sh
for f in async_fetch async_js callback_multi callbacks http interop js regex slots structs; do
  cargo run -- "sample/interop/$f.dream"
done
```

## Run in Node

Each sample has a small `*.mjs` runner (the `interop` and `async_fetch` samples share `runner.mjs`
and `async_runner.mjs`). Build the matching `.wasm` first, then:

```sh
node sample/interop/runner.mjs          # interop.wasm
node sample/interop/async_js.mjs
node sample/interop/callbacks.mjs
node sample/interop/callback_multi.mjs
node sample/interop/js.mjs
node sample/interop/slots.mjs
node sample/interop/structs.mjs
node sample/interop/async_runner.mjs    # async_fetch.wasm (Promise-backed extern async)
node sample/interop/regex.mjs
```

The `smoke.mjs` script builds and runs the deterministic samples end-to-end and is what CI executes:

```sh
node sample/interop/smoke.mjs
```

## Run in the browser

The `*.html` pages import the runtime with `import { run } from "../../runtime/dream.js"`. That path
resolves relative to the page, so **serve the repository root** (not this folder) and open the page
by its path:

```sh
# from the repository root
npx serve .
# then open http://localhost:3000/sample/interop/interop.html
```

Serving `sample/interop/` directly would put `../../runtime/dream.js` above the served root and the
page would 404 on the runtime.

## HTTP / CORS note

`http.dream` and `async_fetch.dream` use the platform `fetch`. In Node there is no CORS restriction,
but in the browser the target endpoint must exist **and** return permissive CORS headers. The
`https://example.com` URLs in `http.dream` are placeholders — repoint them at a CORS-enabled backend
before expecting the browser demo to succeed.
