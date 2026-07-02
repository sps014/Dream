// Node runner for the dynamic `js` interop sample.
//
//   cargo run -- sample/interop/js.dream
//   node sample/interop/js.mjs sample/interop/js.wasm
//
// `js.global("appConfig")` resolves to whatever `globalThis.appConfig` is on the host. The runtime
// registers the object in its handle table and hands Dream a small i32 id; reads, method calls, and
// writes go back through that handle. No per-function glue is needed - the `Dream` host module
// (jsGlobal/jsGetV/jsCallV/...) ships with runtime/dream.js.

import { run } from "../../runtime/dream.js";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const wasmPath = process.argv[2] || here + "js.wasm";

globalThis.appConfig = {
  title: "Hello, Dream",
  count: 7,
  enabled: true,
};

await run(wasmPath);

console.log("appConfig.touched =", globalThis.appConfig.touched);
