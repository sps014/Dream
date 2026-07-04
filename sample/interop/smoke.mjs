// Node smoke test for the Dream JS interop samples. Compiles each deterministic sample through the
// Dream compiler, runs its Node runner, and asserts a known marker appears in the output. This
// guards the JS runtime (runtime/dream.js) against ABI drift with the compiler.
//
//   node sample/interop/smoke.mjs
//
// Requires a built Dream compiler; it shells out to `cargo run -- <file>.dream` to (re)generate the
// git-ignored .wasm/.abi.json artifacts before running each runner. Exits non-zero on any failure.

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..", "..");

// (sample source stem, Node runner, expected substring in the runner's output).
const cases = [
  ["interop", "runner.mjs", "square(7) = 49"],
  ["async_js", "async_js.mjs", "user = user#42"],
  ["async_fetch", "async_runner.mjs", "all = user#1, user#2"],
  ["callbacks", "callbacks.mjs", "[logger] hello from Dream"],
  ["callback_multi", "callback_multi.mjs", "2: c"],
  ["js", "js.mjs", "shout = HELLO, DREAM"],
  ["slots", "slots.mjs", "sum: 60"],
  ["structs", "structs.mjs", "score[1]=8"],
  ["regex", "regex.mjs", ""],
];

let failed = 0;

for (const [stem, runner, expect] of cases) {
  try {
    // Build the .wasm/.abi.json (git-ignored, so may be absent on a fresh checkout).
    execFileSync("cargo", ["run", "--quiet", "--", `sample/interop/${stem}.dream`], {
      cwd: repoRoot,
      stdio: ["ignore", "ignore", "inherit"],
    });
    const out = execFileSync(process.execPath, [path.join(here, runner)], {
      cwd: repoRoot,
      encoding: "utf8",
    });
    if (expect && !out.includes(expect)) {
      console.error(`FAIL ${runner}: expected ${JSON.stringify(expect)} in output:\n${out}`);
      failed++;
    } else {
      console.log(`ok   ${runner}`);
    }
  } catch (e) {
    console.error(`FAIL ${runner}: ${e.message}`);
    if (e.stdout) console.error(String(e.stdout));
    failed++;
  }
}

if (failed) {
  console.error(`\n${failed} interop smoke case(s) failed.`);
  process.exit(1);
}
console.log("\nAll interop smoke cases passed.");
