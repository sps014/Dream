#!/usr/bin/env node
/**
 * Smoke: load a Dream WebWorker program under Node via runtime/dream.js.
 * Usage: node sample/node_webworker_smoke.mjs [path/to.wasm]
 */
import { load } from "../runtime/dream.js";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));
const wasm =
  process.argv[2] ??
  path.join(root, "../tests/cases/webworker_basic.wasm");
const abi = wasm.replace(/\.wasm$/, ".abi.json");

const expected = ["hello!", "world!", "A!", "BB"];
const got = [];

const mod = await load(wasm, {
  abi,
  stdout: (s) => {
    const text = String(s);
    if (text === "\n" || text === "") return;
    const line = text.replace(/\n$/, "");
    process.stdout.write(line + "\n");
    got.push(line);
  },
});

mod.run();

await new Promise((resolve, reject) => {
  const deadline = Date.now() + 15000;
  const tick = () => {
    if (got.length >= expected.length) return resolve();
    if (Date.now() > deadline) {
      return reject(new Error(`timeout waiting for output; got ${JSON.stringify(got)}`));
    }
    setTimeout(tick, 20);
  };
  tick();
});

if (JSON.stringify(got) !== JSON.stringify(expected)) {
  console.error("smoke failed:\n expected", expected, "\n got", got);
  process.exit(1);
}
console.error("node webworker smoke ok");
process.exit(0);
