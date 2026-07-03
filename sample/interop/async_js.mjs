// Node runner for the dynamic-`js` async sample.
//
//   cargo run -- sample/interop/async_js.dream
//   node sample/interop/async_js.mjs sample/interop/async_js.wasm
//
// `fetchUser` returns a Promise resolving to a plain object. Dream calls it via `js.global`,
// receives the Promise as a `js` handle, and `await`s it - the resolved object comes back as a `js`
// whose members are read natively.

import { run } from "../../runtime/dream.js";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const wasmPath = process.argv[2] || here + "async_js.wasm";

globalThis.fetchUser = (id) =>
  new Promise((resolve) => setTimeout(() => resolve({ id, name: `user#${id}` }), 20));

await run(wasmPath);
