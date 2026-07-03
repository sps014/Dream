// Node runner for the struct/class <-> JS object marshaling sample.
//
//   cargo run -- sample/interop/structs.dream
//   node sample/interop/structs.mjs sample/interop/structs.wasm

import { run } from "../../runtime/dream.js";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const wasmPath = process.argv[2] || here + "structs.wasm";

globalThis.receiveUser = (u) => {
  console.log("received:", JSON.stringify(u));
};
globalThis.makeUser = () => ({
  name: "Grace",
  age: 45,
  scores: [7, 8, 9],
  home: { x: 1, y: 2 },
});

await run(wasmPath);
