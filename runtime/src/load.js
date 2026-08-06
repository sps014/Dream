import { TAGS, HEAP_HEADER_SIZE } from "./core.js";
import { DreamInstance } from "./instance.js";
import {
  wrapImport,
  wrapAsyncImport,
  wrapInPlaceByteArrayFill,
  resolveGlobal,
} from "./marshal.js";
import { isNode, setNodeFs, setNodeCrypto } from "./platform.js";
import { defaultEnv } from "./hosts/env.js";
import { defaultDreamModule } from "./hosts.js";
import { csprngBytes } from "./hosts/crypto.js";
import { makeWorkerModule } from "./workers.js";

export { TAGS, HEAP_HEADER_SIZE, DreamInstance };

const FALLBACK_INITIAL_MEMORY_PAGES = 64;
const FALLBACK_MAX_MEMORY_PAGES = 65536;

async function fetchBytes(source) {
  if (source instanceof ArrayBuffer) return new Uint8Array(source);
  if (source instanceof Uint8Array) return source;
  if (!isNode && typeof fetch === "function") {
    const res = await fetch(source);
    if (!res.ok) throw new Error(`failed to fetch ${source}: ${res.status}`);
    return new Uint8Array(await res.arrayBuffer());
  }
  const { readFile } = await import("node:fs/promises");
  return new Uint8Array(await readFile(source));
}

async function loadAbi(abi) {
  if (!abi) return null;
  if (typeof abi === "object" && abi.externs) return abi;
  const bytes = await fetchBytes(abi);
  return JSON.parse(new TextDecoder("utf-8").decode(bytes));
}

function makeLinearMemory(wasmModule) {
  const memoryImport = WebAssembly.Module.imports(wasmModule).find(
    (i) => i.module === "env" && i.name === "memory" && i.kind === "memory",
  );
  const desc = memoryImport && memoryImport.type;
  return new WebAssembly.Memory({
    initial: desc ? desc.minimum : FALLBACK_INITIAL_MEMORY_PAGES,
    maximum: desc ? desc.maximum : FALLBACK_MAX_MEMORY_PAGES,
    shared: desc ? desc.shared : true,
  });
}

/**
 * Loads and instantiates a Dream module.
 *
 * @param {string|ArrayBuffer|Uint8Array} source - URL/path to `.wasm`, or raw bytes.
 * @param {object} [options]
 * @param {object} [options.imports] - JS implementations keyed by extern function name.
 * @param {string|object} [options.abi] - URL/path to (or parsed) `.abi.json` for auto-marshaling.
 * @param {function} [options.stdout] - Custom output sink for print builtins.
 * @param {function} [options.dreamHosts] - Optional host-factory composer; defaults to full module.
 * @returns {Promise<DreamInstance>}
 */
export async function load(source, options = {}) {
  const wasmBytes = await fetchBytes(source);
  const abi = await loadAbi(options.abi);
  const wasmModule = await WebAssembly.compile(wasmBytes);

  if (isNode && !options.__skipNodePreload) {
    try {
      const fs = await import("node:fs");
      setNodeFs(fs);
    } catch (_) { /* leave unavailable */ }
    try {
      const crypto = await import("node:crypto");
      setNodeCrypto(crypto);
    } catch (_) { /* leave unavailable */ }
  }

  let instance = null;
  const getInstance = () => {
    if (!instance) throw new Error("instance not ready");
    return instance;
  };

  const importObject = { env: defaultEnv(getInstance, options) };
  const sharedMemory = options.memory ?? makeLinearMemory(wasmModule);
  importObject.env.memory = sharedMemory;

  const userImports = options.imports || {};
  const sigByName = new Map();
  if (abi) for (const e of abi.externs) sigByName.set(e.name, e);

  const composeHosts = options.dreamHosts || defaultDreamModule;
  const builtinDream = {
    ...composeHosts(getInstance),
    ...makeWorkerModule(wasmBytes, abi, () => sharedMemory),
  };
  if (typeof builtinDream.__attachGpuAbi === "function") {
    const hint =
      typeof source === "string"
        ? source
        : typeof options.abi === "string"
          ? options.abi
          : null;
    builtinDream.__attachGpuAbi(abi, hint);
  }

  const wrapFor = (fn, sig) => {
    if (sig && sig.field === "cryptoSecureRandomFill") {
      return wrapInPlaceByteArrayFill(getInstance, (count) => csprngBytes(count));
    }
    return sig && sig.async ? wrapAsyncImport(getInstance, fn, sig) : wrapImport(getInstance, fn, sig);
  };

  for (const name of Object.keys(userImports)) {
    const sig = sigByName.get(name);
    const module = sig ? sig.module : "env";
    const field = sig ? sig.field : name;
    (importObject[module] ||= {})[field] = wrapFor(userImports[name], sig);
  }

  if (abi) {
    for (const e of abi.externs) {
      const bucket = (importObject[e.module] ||= {});
      if (bucket[e.field]) continue;
      const resolved = (e.module === "Dream" && builtinDream[e.field])
        ? builtinDream[e.field]
        : resolveGlobal(e.module, e.field);
      bucket[e.field] = resolved
        ? wrapFor(resolved, e)
        : () => {
            throw new Error(`no JS implementation for extern '${e.name}' (${e.module}.${e.field})`);
          };
    }
  }

  const wasmInstance = await WebAssembly.instantiate(wasmModule, importObject);
  instance = new DreamInstance(wasmInstance);
  return instance;
}

/**
 * load a module and immediately invoke its `main`.
 *
 * @returns {Promise<DreamInstance>} the loaded instance (after `main` has run).
 */
export async function run(source, options = {}) {
  const abi =
    options.abi ?? (typeof source === "string" ? source.replace(/\.wasm$/, ".abi.json") : undefined);
  const mod = await load(source, { ...options, abi });
  mod.run();
  return mod;
}

export default { load, run, DreamInstance, TAGS, HEAP_HEADER_SIZE };
