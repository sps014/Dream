// Node runner for the multi-argument callback sample.
//
//   cargo run -- sample/interop/callback_multi.dream
//   node sample/interop/callback_multi.mjs sample/interop/callback_multi.wasm

import { run } from "../../runtime/dream.js";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const wasmPath = process.argv[2] || here + "callback_multi.wasm";

globalThis.api = {
  forEach2(cb) {
    ["a", "b", "c"].forEach((value, index) => cb(value, index));
  },
};

await run(wasmPath);
