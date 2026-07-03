// Node runner for the shadow-stack slot-marshaling sample.
//
//   cargo run -- sample/interop/slots.dream
//   node sample/interop/slots.mjs sample/interop/slots.wasm

import { run } from "../../runtime/dream.js";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const wasmPath = process.argv[2] || here + "slots.wasm";

globalThis.api = {
  log(...args) {
    console.log("log:", JSON.stringify(args), "types:", args.map((a) => typeof a).join(","));
  },
  sum(arr) {
    console.log("sum:", arr.reduce((a, b) => a + b, 0), "of", JSON.stringify(arr));
  },
  each(fn) {
    ["a", "b"].forEach(fn);
  },
};

await run(wasmPath);
