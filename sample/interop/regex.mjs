// Node runner for the regex sample.
//
//   cargo run -- sample/interop/regex.dream
//   node sample/interop/regex.mjs sample/interop/regex.wasm
//
// Regex needs no custom imports: the engine is plain compiled Dream (see
// src/stdlib/text/regex*.dream), not a host binding, so there is nothing runtime-specific to wire
// up here.

import { run } from "../../runtime/dream.js";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const wasmPath = process.argv[2] || here + "regex.wasm";

await run(wasmPath);
