// Dream JS interop runtime.
//
// Loads a Dream-compiled `.wasm` module, wires the default `env` builtins, and lets you bind
// JavaScript implementations to `extern fun` declarations with automatic value marshaling for
// strings, arrays, `List<T>`, and structs. Works as an ES module in both the browser and Node.
//
// Usage (browser):
//   import { load } from "./dream.js";
//   const mod = await load("interop.wasm", {
//     abi: "interop.abi.json",            // optional; enables auto-marshaling of imports
//     imports: { alert: (msg) => window.alert(msg) },
//   });
//   mod.run();                            // calls exported `main`
//
// Usage (Node >= 18):
//   import { load } from "./dream.js";
//   const mod = await load("interop.wasm", { imports: { alert: console.log } });
//   mod.run();
// WebWorker / WebWorkerPool also work under Node via `worker_threads` (see
// `sample/node_webworker_smoke.mjs`).

// Runtime type tags stored in each heap block header (see object.rs).
export const TAGS = {
  INT: 1,
  FLOAT: 2,
  DOUBLE: 3,
  BOOL: 4,
  STRING: 5,
  ARRAY: 6,
  CHAR: 7,
  LONG: 8,
  UINT: 9,
  ULONG: 10,
  BYTE: 11,
  STRUCT_BASE: 12,
};

// Byte size of the universal heap-block header: [size:i32][tag:i32][ref_count:i32].
// Allocated pointers point at `data` (block_start + HEAP_HEADER_SIZE).
export const HEAP_HEADER_SIZE = 12;

/** Byte size of a single element of the given Dream type (see utils.rs `element_size_of`). */
function elementSize(typeName) {
  if (typeName === "bool" || typeName === "char" || typeName === "byte") return 1;
  if (typeName === "double" || typeName === "long" || typeName === "ulong") return 8;
  return 4; // int, uint, float, and every reference type (pointer)
}

/** Strips a trailing `?` (nullable) and `[]` (array) suffix from a type name. */
function stripSuffix(typeName) {
  let t = typeName;
  if (t.endsWith("?")) t = t.slice(0, -1);
  return t;
}

const isPrimitive = (t) => t === "int" || t === "float" || t === "double" || t === "bool";

/** True for a Dream function type string like `fun(int,string):void`. */
const isFunType = (t) => typeof t === "string" && t.startsWith("fun(");

/** Parses `fun(p1,p2):ret` into `{ params: [...], result }`. */
function parseFunType(typeStr) {
  const open = typeStr.indexOf("(");
  const close = typeStr.lastIndexOf(")");
  const inner = typeStr.slice(open + 1, close).trim();
  const result = typeStr.slice(close + 1).replace(/^:/, "").trim() || "void";
  const params = inner.length ? inner.split(",").map((s) => s.trim()) : [];
  return { params, result };
}

/** Marshals a JS value into the raw WASM value for Dream type `t` (used for callback args/results). */
function jsToWasm(inst, t, value) {
  const base = stripSuffix(t);
  if (base === "string") return inst.writeString(value == null ? "" : String(value));
  if (base === "bool") return value ? 1 : 0;
  if (base === "js") return inst.registerHandle(value);
  if (base === "void") return 0;
  // `long`/`ulong` are wasm i64, which the JS-WASM boundary represents as BigInt.
  if (base === "long" || base === "ulong") return BigInt(value == null ? 0 : value);
  return value == null ? 0 : value; // numeric primitive or opaque pointer
}

/** Marshals a raw WASM value back into a JS value for Dream type `t`. */
function wasmToJs(inst, t, raw) {
  const base = stripSuffix(t);
  if (base === "string") return inst.readString(raw);
  if (base === "bool") return raw !== 0;
  if (base === "js") return inst.derefHandle(raw);
  if (base === "void") return undefined;
  return raw;
}

/**
 * A loaded Dream module instance. Exposes the raw WASM exports plus helpers that understand
 * Dream's heap layout so you can read/write strings, arrays, lists, and structs.
 */
export class DreamInstance {
  constructor(instance) {
    this.instance = instance;
    this.exports = instance.exports;
    this.memory = instance.exports.memory;
    // JS-object handle registry backing the Dream `js` type. A `js` value crosses the boundary
    // as a small i32 id; the host keeps the real JS value here. Id 0 is reserved for null.
    this._jsHandles = new Map(); // id -> JS value
    this._jsIds = new Map(); // JS value -> id (identity for objects, value for primitives)
    this._jsNextId = 1;
    this._jsFreeIds = [];
    // Cache of JS callables wrapping Dream funcrefs, keyed by `${index}|${typeStr}`. Funcrefs are
    // captureless table indices, so index identity == function identity: returning the *same* JS
    // callable for the same funcref lets `addEventListener`/`removeEventListener` pair correctly.
    this._callbackWrappers = new Map();
  }

  /** Registers a JS value, returning its `js` handle id (0 for null/undefined). Idempotent per value. */
  registerHandle(value) {
    if (value === null || value === undefined) return 0;
    const existing = this._jsIds.get(value);
    if (existing !== undefined) return existing;
    const id = this._jsFreeIds.length ? this._jsFreeIds.pop() : this._jsNextId++;
    this._jsHandles.set(id, value);
    this._jsIds.set(value, id);
    return id;
  }

  /** Resolves a `js` handle id back to its JS value (null for id 0 / unknown). */
  derefHandle(id) {
    if (!id) return null;
    return this._jsHandles.has(id) ? this._jsHandles.get(id) : null;
  }

  /** Releases the handle for `value` so its id can be reused and the JS value can be collected. */
  releaseValue(value) {
    if (value === null || value === undefined) return;
    const id = this._jsIds.get(value);
    if (id === undefined) return;
    this._jsHandles.delete(id);
    this._jsIds.delete(value);
    this._jsFreeIds.push(id);
  }

  /** A fresh DataView over current memory (memory may grow, so do not cache the buffer). */
  get view() {
    return new DataView(this.memory.buffer);
  }

  /** A fresh Uint8Array over current memory. */
  get bytes() {
    return new Uint8Array(this.memory.buffer);
  }

  // --- raw scalar reads -----------------------------------------------------
  i32(ptr) {
    return this.view.getInt32(ptr, true);
  }
  f32(ptr) {
    return this.view.getFloat32(ptr, true);
  }
  f64(ptr) {
    return this.view.getFloat64(ptr, true);
  }

  /**
   * Reads a Dream string at `ptr` (a data pointer). Layout: `[len: i32][utf8...]`, so the length
   * prefix gives the byte count directly (no NUL terminator).
   */
  readString(ptr) {
    if (!ptr) return "";
    const bytes = this.bytes;
    const len = this.view.getInt32(ptr, true);
    const start = ptr + 4;
    // `TextDecoder.decode` rejects views backed by a `SharedArrayBuffer` (linear memory is always
    // shared now — see `makeLinearMemory`), so copy the slice into a plain, non-shared buffer first.
    return new TextDecoder("utf-8").decode(bytes.slice(start, start + len));
  }

  /**
   * Allocates a Dream string block for `str` and returns its data pointer, so JS-implemented
   * extern functions can return strings back into Dream. Requires the module to export `malloc`.
   * Layout: `[len: i32][utf8...]` (no NUL terminator).
   */
  writeString(str) {
    if (typeof this.exports.malloc !== "function") {
      throw new Error("module does not export `malloc`; cannot allocate a string");
    }
    const encoded = new TextEncoder().encode(str);
    const ptr = this.exports.malloc(4 + encoded.length, TAGS.STRING);
    const bytes = this.bytes;
    this.view.setInt32(ptr, encoded.length, true); // length prefix
    bytes.set(encoded, ptr + 4);
    return ptr;
  }

  /** Reads a single element of `elemType` at byte address `addr`. */
  _readElement(addr, elemType) {
    const t = stripSuffix(elemType);
    switch (t) {
      case "int":
        return this.i32(addr);
      case "char":
      case "byte":
        return this.bytes[addr]; // 1-byte element
      case "bool":
        return this.bytes[addr] !== 0;
      case "uint":
        return this.view.getUint32(addr, true);
      case "long":
        return this.view.getBigInt64(addr, true);
      case "ulong":
        return this.view.getBigUint64(addr, true);
      case "float":
        return this.f32(addr);
      case "double":
        return this.f64(addr);
      case "string":
        return this.readString(this.i32(addr));
      default:
        if (t.endsWith("[]")) return this.readArray(this.i32(addr), t.slice(0, -2));
        return this.i32(addr); // struct/object/list: opaque pointer
    }
  }

  /** Writes a single element of `elemType` at byte address `addr`. */
  _writeElement(addr, elemType, value) {
    const t = stripSuffix(elemType);
    switch (t) {
      case "int":
        this.view.setInt32(addr, value | 0, true);
        break;
      case "char":
      case "byte":
        this.bytes[addr] = value & 0xff; // 1-byte element
        break;
      case "bool":
        this.bytes[addr] = value ? 1 : 0;
        break;
      case "uint":
        this.view.setUint32(addr, value >>> 0, true);
        break;
      case "long":
        this.view.setBigInt64(addr, BigInt(value == null ? 0 : value), true);
        break;
      case "ulong":
        this.view.setBigUint64(addr, BigInt(value == null ? 0 : value), true);
        break;
      case "float":
        this.view.setFloat32(addr, value, true);
        break;
      case "double":
        this.view.setFloat64(addr, value, true);
        break;
      case "string":
        this.view.setInt32(addr, this.writeString(value == null ? "" : String(value)), true);
        break;
      default:
        this.view.setInt32(addr, value | 0, true); // struct/object pointer
    }
  }

  /**
   * Allocates a Dream array from a JS array (or typed array) of `elemType`, returning its data
   * pointer, so JS-implemented externs can return arrays (e.g. `char[]` file bytes) back into
   * Dream. Layout: [count:i32] followed by `count` elements. Requires the module to export `malloc`.
   */
  writeArray(arr, elemType = "int") {
    if (typeof this.exports.malloc !== "function") {
      throw new Error("module does not export `malloc`; cannot allocate an array");
    }
    const elem = stripSuffix(elemType);
    const size = elementSize(elem);
    const count = arr.length;
    const ptr = this.exports.malloc(4 + count * size, TAGS.ARRAY);
    this.view.setInt32(ptr, count, true);
    if (elem === "char" || elem === "byte") {
      // Bulk copy for the common byte-array case.
      this.bytes.set(Uint8Array.from(arr), ptr + 4);
    } else {
      for (let i = 0; i < count; i++) {
        this._writeElement(ptr + 4 + i * size, elem, arr[i]);
      }
    }
    return ptr;
  }

  /**
   * Reads a Dream array at data pointer `ptr` into a JS array. Layout: [count:i32] followed by
   * `count` elements of `elemType`.
   */
  readArray(ptr, elemType = "int") {
    if (!ptr) return [];
    const count = this.i32(ptr);
    const size = elementSize(elemType);
    const out = new Array(count);
    for (let i = 0; i < count; i++) {
      out[i] = this._readElement(ptr + 4 + i * size, elemType);
    }
    return out;
  }

  /**
   * Reads a `List<T>` at data pointer `ptr` into a JS array. A List is a struct `{ items: T[];
   * count: int }`, so `items` is at offset 0 and the logical length at offset 4.
   */
  readList(ptr, elemType = "int") {
    if (!ptr) return [];
    const itemsPtr = this.i32(ptr);
    const count = this.i32(ptr + 4);
    const size = elementSize(elemType);
    const out = new Array(count);
    for (let i = 0; i < count; i++) {
      out[i] = this._readElement(itemsPtr + 4 + i * size, elemType);
    }
    return out;
  }

  /**
   * Reads a struct at data pointer `ptr` using a schema describing its fields in declaration
   * order. Schema entries are `{ name, type }`; offsets are derived from element sizes.
   */
  readStruct(ptr, schema) {
    const out = {};
    let offset = 0;
    for (const field of schema) {
      out[field.name] = this._readElement(ptr + offset, field.type);
      offset += elementSize(field.type);
    }
    return out;
  }

  /**
   * Wraps a Dream function value (an `i32` index into the exported `__indirect_function_table`)
   * as a JS callable, so a Dream function passed to a `fun(...)`-typed extern parameter can be
   * invoked by the host. `typeStr` is the Dream function type (e.g. `fun(int):void`) used to
   * marshal arguments in and the result out.
   */
  callback(index, typeStr = "fun():void") {
    if (index < 0) return null;
    const table = this.exports.__indirect_function_table;
    if (!table) throw new Error("module does not export its function table; cannot build a callback");
    const cacheKey = `${index}|${typeStr}`;
    const cached = this._callbackWrappers.get(cacheKey);
    if (cached) return cached;
    const fn = table.get(index);
    if (typeof fn !== "function") {
      throw new Error(`no Dream function at table index ${index}`);
    }
    const { params, result } = parseFunType(typeStr);
    const wrapper = (...jsArgs) => {
      const raw = params.map((p, i) => jsToWasm(this, p, jsArgs[i]));
      const out = fn(...raw);
      return wasmToJs(this, result, out);
    };
    // Expose the raw table index so callers that need the portable funcref value itself (e.g.
    // `WebWorker` shipping a body to another instance of the same module) can recover it.
    wrapper.__dreamFuncIndex = index;
    this._callbackWrappers.set(cacheKey, wrapper);
    return wrapper;
  }

  /**
   * Runs one `fun(string): string` worker body: writes `msg` into this instance's memory, calls the
   * exported `__dream_worker_invoke_raw` trampoline (publishes `env` to the closure-env global, then
   * a single `call_indirect` on the body funcref `fnIndex`), and resolves with the reply string.
   * Used by the Web Worker / Node worker_threads bootstrap. Returns a Promise because an *async*
   * body's `call_indirect` only returns its `Future` constructor's frame pointer, not the real
   * reply — see `__awaitWorkerResult`.
   *
   * Workers (browser and Node) import the parent's shared `WebAssembly.Memory`, so an `@shared
   * class` / unmanaged `env` pointer is meaningful across threads. Browser pages still need
   * COOP/COEP headers for `SharedArrayBuffer`; Node's `worker_threads` exposes it by default.
   */
  __workerInvoke(fnIndex, env, msg) {
    const ptr = this.writeString(msg == null ? "" : String(msg));
    const r = this.exports.__dream_worker_invoke_raw(fnIndex, env, ptr);
    return this.__awaitWorkerResult(r);
  }

  /**
   * Interprets a `__dream_worker_invoke_raw` return value, mirroring the tag check the *native*
   * `__dream_worker_invoke` trampoline does synchronously in WASM (`src/mir/emit/module.rs`): every
   * heap allocation carries a tag except a `Future` frame (`dream_new_future` mallocs with tag `0`,
   * a value no real Dream type uses), so an untagged, non-null `r` means the body was an `async
   * fun` and `r` is its still-running task, not the reply yet.
   *
   * Unlike native (where every host `async` op resolves synchronously before `call_indirect`
   * returns, so one `__dream_run_loop` pass always finishes the task), a real `extern async` host
   * call here only settles later via a Promise `.then()` callback (see `wrapAsyncImport`), which
   * itself re-pumps `__dream_run_loop`. So instead of draining once, poll the future's `F_STATUS`
   * slot on the microtask queue until some pump marks it done, then unwrap `F_RESULT`.
   */
  __awaitWorkerResult(r) {
    const F_STATUS = 4; // mirrors src/mir/async_emit.rs
    const F_RESULT = 8;
    const tag = r === 0 ? 0 : this.i32(r - 8); // mirrors the WASM `$object_tag` helper
    if (tag !== 0) return Promise.resolve(this.readString(r));
    return new Promise((resolve) => {
      // `setTimeout`, not `queueMicrotask`: a real pending host op (e.g. `fetch`) settles via a
      // macrotask-queued I/O callback, which a tight microtask-only poll loop would starve (the
      // microtask queue must fully drain before the next macrotask runs), hanging forever.
      const poll = () => {
        if (this.i32(r + F_STATUS) !== 0) {
          resolve(this.readString(this.i32(r + F_RESULT)));
        } else {
          setTimeout(poll, 0);
        }
      };
      poll();
    });
  }

  /** Calls the exported `main`, if present. Returns its result (if any). */
  run() {
    if (typeof this.exports.main === "function") {
      return this.exports.main();
    }
    throw new Error("module has no exported `main`");
  }
}

/** Marshals raw WASM argument values into JS values per the parameter type names. */
function marshalArgs(inst, params, rawArgs) {
  if (!params) return rawArgs;
  return rawArgs.map((arg, i) => {
    const rawType = params[i] || "int";
    if (isFunType(rawType)) return inst.callback(arg, rawType); // Dream fn index -> JS callable
    const t = stripSuffix(rawType);
    if (t === "string") return inst.readString(arg);
    if (t === "js") return inst.derefHandle(arg); // i32 handle id -> live JS value
    if (t.endsWith("[]")) return inst.readArray(arg, t.slice(0, -2));
    if (t === "bool") return arg !== 0;
    return arg; // numeric primitive or opaque pointer
  });
}

/** Marshals a JS return value back into the raw WASM value for the declared result type. */
function marshalResult(inst, result, ret) {
  if (result === "string") return inst.writeString(ret == null ? "" : String(ret));
  if (result === "bool") return ret ? 1 : 0;
  if (result === "js") return inst.registerHandle(ret); // live JS value -> i32 handle id
  if (typeof result === "string" && result.endsWith("[]")) {
    return inst.writeArray(ret == null ? [] : ret, result.slice(0, -2)); // e.g. char[] file bytes
  }
  if (result === "void" || result == null) return ret == null ? 0 : ret;
  return ret;
}

/** True when marshaling `t` needs the live `DreamInstance` (strings, `js`, arrays, callbacks). */
function typeNeedsInstance(t) {
  if (!t || t === "void") return false;
  if (isFunType(t)) return true;
  const base = stripSuffix(t);
  return base === "string" || base === "js" || base.endsWith("[]");
}

/** True when any param/result of an extern needs heap marshaling via `getInstance()`. */
function signatureNeedsInstance(params, result) {
  if (typeNeedsInstance(result)) return true;
  if (params) {
    for (const p of params) {
      if (typeNeedsInstance(p)) return true;
    }
  }
  return false;
}

/** Wraps a user-provided import implementation so its args/return are marshaled per the ABI. */
function wrapImport(getInstance, fn, signature) {
  const params = signature ? signature.params : null;
  const result = signature ? signature.result : null;
  // `(start $__runtime_init)` runs during `WebAssembly.instantiate`, before `DreamInstance` exists.
  // Pure numeric externs (e.g. `gpuBufferAllocBytes`) must not call `getInstance()` or module-level
  // constructors that touch the host die with "instance not ready".
  const needsInst = signatureNeedsInstance(params, result);

  return (...rawArgs) => {
    if (!needsInst) {
      const ret = fn(...rawArgs);
      return ret == null ? 0 : ret;
    }
    const inst = getInstance();
    const args = marshalArgs(inst, params, rawArgs);
    const ret = fn(...args);
    return marshalResult(inst, result, ret);
  };
}

/**
 * In-place `byte[]` fill imports receive a data pointer, not a marshaled copy. Writes random bytes
 * directly into linear memory at `destPtr` (layout: `[count:i32][bytes...]`).
 */
function wrapInPlaceByteArrayFill(getInstance, fillBytes) {
  return (destPtr) => {
    const inst = getInstance();
    const count = inst.i32(destPtr);
    if (count > 0) {
      inst.bytes.set(fillBytes(count), destPtr + 4);
    }
    return 0;
  };
}

// Future heap kinds/sizes (mirrors src/mir/async_emit.rs).
const FUTURE_KIND_HOST = 1;
const FUTURE_SLOTS_SIZE = 56; // F_SLOTS: a host future has no saved-locals region.

/**
 * Wraps an `extern async` import. The JS implementation returns a Promise; the wrapper
 * synchronously allocates a host `Future` and hands its pointer back to Dream, then resolves it
 * (and re-pumps the scheduler) once the Promise settles. This is the only place the JS `.then`
 * bridge lives - Dream source never sees a Promise.
 */
function wrapAsyncImport(getInstance, fn, signature) {
  const params = signature ? signature.params : null;
  const result = signature ? signature.result : null;

  return (...rawArgs) => {
    const inst = getInstance();
    const exports = inst.exports;
    if (typeof exports.__dream_new_future !== "function") {
      throw new Error("module does not export the async runtime; cannot bridge an extern async import");
    }
    const args = marshalArgs(inst, params, rawArgs);
    const future = exports.__dream_new_future(FUTURE_SLOTS_SIZE, -1, FUTURE_KIND_HOST);
    Promise.resolve(fn(...args)).then(
      (value) => {
        exports.__dream_resolve(future, marshalResult(inst, result, value));
        exports.__dream_run_loop();
      },
      (err) => {
        // A rejected Promise has no Dream-level error channel yet; settle the future with a
        // zero/null result (a `null` `js` handle for a `js` result) so the scheduler is not left
        // hanging, and surface the reason on the console for diagnosis.
        console.error("Dream: awaited JS promise rejected:", err);
        exports.__dream_resolve(future, marshalResult(inst, result, null));
        exports.__dream_run_loop();
      },
    );
    return future;
  };
}

/**
 * Resolves an extern import against the JS global scope so common APIs need no boilerplate.
 * The `env` module maps to a bare global (e.g. `alert`); any other module maps to a property of
 * that global object (e.g. `console.log`, `Math.max`). Returns the function bound to its owner,
 * or `undefined` if there is no matching global function.
 */
function resolveGlobal(module, field) {
  if (module === "env") {
    const g = globalThis[field];
    return typeof g === "function" ? g.bind(globalThis) : undefined;
  }
  const owner = globalThis[module];
  const fn = owner && owner[field];
  return typeof fn === "function" ? fn.bind(owner) : undefined;
}

/**
 * Performs one HTTP request via the platform `fetch` and serializes the whole response into a single
 * `Uint8Array` for `src/stdlib/net/http_response.dream`: an ASCII head (status line + header lines) and a blank
 * line, then the raw body bytes. Keeping the body raw (an `arrayBuffer`) makes binary responses
 * byte-exact. `body` is either a string or a `Uint8Array` (or "" / empty for none).
 */
async function httpDo(url, method, headersJson, body, timeoutMs) {
  const verb = (method || "GET").toUpperCase();
  const init = { method: verb };
  if (headersJson && headersJson !== "") {
    try { init.headers = JSON.parse(headersJson); } catch (_) { /* ignore bad header json */ }
  }
  const hasBody = typeof body === "string" ? body !== "" : body && body.length > 0;
  if (hasBody && verb !== "GET" && verb !== "HEAD") {
    init.body = body;
  }
  const ms = Number(timeoutMs) || 0;
  if (ms > 0) {
    if (typeof AbortSignal !== "undefined" && typeof AbortSignal.timeout === "function") {
      init.signal = AbortSignal.timeout(ms);
    } else {
      const ctrl = new AbortController();
      init.signal = ctrl.signal;
      setTimeout(() => ctrl.abort(), ms);
    }
  }
  try {
    const res = await fetch(url, init);

    let head = `${res.status}\n`;
    res.headers.forEach((value, name) => {
      head += `${name}: ${value}\n`;
    });
    head += "\n"; // blank line separating head from body
    const headBytes = new TextEncoder().encode(head);
    const bodyBytes = new Uint8Array(await res.arrayBuffer());

    const out = new Uint8Array(headBytes.length + bodyBytes.length);
    out.set(headBytes, 0);
    out.set(bodyBytes, headBytes.length);
    return out;
  } catch (e) {
    const msg = (e && (e.name === "TimeoutError" || e.name === "AbortError"))
      ? "timeout"
      : String((e && e.message) || e || "fetch failed");
    const headBytes = new TextEncoder().encode("0\n\n");
    const bodyBytes = new TextEncoder().encode(msg);
    const out = new Uint8Array(headBytes.length + bodyBytes.length);
    out.set(headBytes, 0);
    out.set(bodyBytes, headBytes.length);
    return out;
  }
}

// Slot tags for the dynamic-`js` argument buffer. One argument = one 16-byte slot laid out as
// `[tag: i32][aux: i32][payload: 8 bytes]`. Must match `src/mir/emit/types.rs::js_slot`.
const JS_SLOT = {
  NULL: 0, INT: 1, LONG: 2, DOUBLE: 3, BOOL: 4, STRING: 5, JS: 6, FUNC: 7, ARRAY: 8,
};
// Maps an array element's slot tag (the `aux` word of an ARRAY slot) to the Dream element-type name
// understood by `readArray`.
const JS_ARRAY_ELEM = {
  [JS_SLOT.INT]: "int", [JS_SLOT.LONG]: "long", [JS_SLOT.DOUBLE]: "double",
  [JS_SLOT.BOOL]: "bool", [JS_SLOT.STRING]: "string", [JS_SLOT.JS]: "js",
};

/**
 * Decodes `argc` tagged argument slots starting at `ptr` (in the instance's linear memory) into an
 * array of live JS values. Primitives are read in place, strings/arrays are materialized, `js`
 * handles are dereferenced, and Dream funcrefs are wrapped as identity-stable JS callables. This is
 * the read side of the shadow-stack marshaling emitted by `Emitter::emit_js_call`.
 */
function decodeJsSlots(inst, ptr, argc) {
  const dv = inst.view;
  const out = new Array(argc);
  for (let i = 0; i < argc; i++) {
    const base = ptr + i * 16;
    const tag = dv.getInt32(base, true);
    const aux = dv.getInt32(base + 4, true);
    const p = base + 8;
    switch (tag) {
      case JS_SLOT.NULL: out[i] = null; break;
      case JS_SLOT.INT: out[i] = dv.getInt32(p, true); break;
      case JS_SLOT.LONG: out[i] = dv.getBigInt64(p, true); break;
      case JS_SLOT.DOUBLE: out[i] = dv.getFloat64(p, true); break;
      case JS_SLOT.BOOL: out[i] = dv.getInt32(p, true) !== 0; break;
      case JS_SLOT.STRING: out[i] = inst.readString(dv.getInt32(p, true)); break;
      case JS_SLOT.JS: out[i] = inst.derefHandle(dv.getInt32(p, true)); break;
      case JS_SLOT.FUNC: {
        // `aux` is the callback's parameter count; each parameter is passed as a `js` handle and the
        // result is discarded (`void`), so reconstruct `fun(js, js, …): void` of the right arity.
        const arity = aux > 0 ? aux : 0;
        const sig = `fun(${Array(arity).fill("js").join(",")}):void`;
        out[i] = inst.callback(dv.getInt32(p, true), sig);
        break;
      }
      case JS_SLOT.ARRAY: {
        const arrPtr = dv.getInt32(p, true);
        const elem = JS_ARRAY_ELEM[aux] || "int";
        out[i] = elem === "js"
          ? inst.readArray(arrPtr, "int").map((h) => inst.derefHandle(h))
          : inst.readArray(arrPtr, elem);
        break;
      }
      default: out[i] = null;
    }
  }
  return out;
}

/**
 * The built-in `Dream` host module backing the stdlib interop layer (the dynamic `js` type, regex,
 * fetch). These run *after* argument marshaling, so a `js` parameter arrives as the live JS value
 * and a `js`/`string`/number result is marshaled back automatically. Only `jsRelease` needs the
 * instance, to drop the handle for the value it was given.
 */
function defaultDreamModule(getInstance) {
  return {
    ...makeGpuHost(getInstance),
    // --- entry points / value constructors --------------------------------
    jsGlobal: (name) => globalThis[name],
    jsGlobalThis: () => globalThis,
    jsObject: () => ({}),
    jsArray: () => [],
    // Boxing: a Dream primitive/string -> a `js` handle (marshalResult registers the return value).
    jsString: (value) => value,
    jsInt: (value) => value,
    jsLong: (value) => value,
    jsDouble: (value) => value,
    jsBool: (value) => value,
    // --- dynamic operations (deferred binding happens right here in JS) ----
    jsGetV: (target, name) => (target == null ? undefined : target[name]),
    jsSetV: (target, name, value) => { if (target != null) target[name] = value; },
    // Property write with one shadow-stack value slot (no pre-box bridge for primitives/string).
    jsSetSlot: (target, name, argsPtr, argc) => {
      const [value] = decodeJsSlots(getInstance(), argsPtr, argc);
      if (target != null) target[name] = value;
    },
    // Variadic call/invoke. Arguments arrive as a shadow-stack buffer of tagged 16-byte slots
    // (`argsPtr`/`argc`); `decodeJsSlots` reads them straight out of linear memory - one boundary
    // crossing, no per-argument handles.
    jsCallV: (target, name, argsPtr, argc) =>
      target[name](...decodeJsSlots(getInstance(), argsPtr, argc)),
    jsInvokeV: (target, argsPtr, argc) =>
      target(...decodeJsSlots(getInstance(), argsPtr, argc)),
    // Fused `target[prop][method](...args)` — one crossing instead of get + call.
    jsGetCallV: (target, prop, method, argsPtr, argc) => {
      const recv = target == null ? undefined : target[prop];
      return recv[method](...decodeJsSlots(getInstance(), argsPtr, argc));
    },
    jsIndexGetV: (target, key) => (target == null ? undefined : target[key]),
    jsIndexSetV: (target, key, value) => { if (target != null) target[key] = value; },
    // Index write with key+value in two shadow-stack slots.
    jsIndexSetSlot: (target, argsPtr, argc) => {
      const [key, value] = decodeJsSlots(getInstance(), argsPtr, argc);
      if (target != null) target[key] = value;
    },
    // Typed-boundary fusion: get that returns a Dream primitive/string (no intermediate handle).
    jsGetAsInt: (target, name) => {
      const v = target == null ? undefined : target[name];
      return v == null ? 0 : (Number(v) | 0);
    },
    jsGetAsLong: (target, name) => {
      const v = target == null ? undefined : target[name];
      return v == null ? 0 : Math.trunc(Number(v));
    },
    jsGetAsDouble: (target, name) => {
      const v = target == null ? undefined : target[name];
      return v == null ? 0 : Number(v);
    },
    jsGetAsBool: (target, name) => !!(target == null ? undefined : target[name]),
    jsGetAsString: (target, name) => {
      const v = target == null ? undefined : target[name];
      return v == null ? "" : String(v);
    },
    // Typed-boundary fusion: call / invoke returning a Dream primitive/string.
    jsCallAsInt: (target, name, argsPtr, argc) => {
      const v = target[name](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : (Number(v) | 0);
    },
    jsCallAsLong: (target, name, argsPtr, argc) => {
      const v = target[name](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : Math.trunc(Number(v));
    },
    jsCallAsDouble: (target, name, argsPtr, argc) => {
      const v = target[name](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : Number(v);
    },
    jsCallAsBool: (target, name, argsPtr, argc) =>
      !!target[name](...decodeJsSlots(getInstance(), argsPtr, argc)),
    jsCallAsString: (target, name, argsPtr, argc) => {
      const v = target[name](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? "" : String(v);
    },
    jsInvokeAsInt: (target, argsPtr, argc) => {
      const v = target(...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : (Number(v) | 0);
    },
    jsInvokeAsLong: (target, argsPtr, argc) => {
      const v = target(...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : Math.trunc(Number(v));
    },
    jsInvokeAsDouble: (target, argsPtr, argc) => {
      const v = target(...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : Number(v);
    },
    jsInvokeAsBool: (target, argsPtr, argc) =>
      !!target(...decodeJsSlots(getInstance(), argsPtr, argc)),
    jsInvokeAsString: (target, argsPtr, argc) => {
      const v = target(...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? "" : String(v);
    },
    // Fused get + call + typed unbox.
    jsGetCallAsInt: (target, prop, method, argsPtr, argc) => {
      const recv = target == null ? undefined : target[prop];
      const v = recv[method](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : (Number(v) | 0);
    },
    jsGetCallAsLong: (target, prop, method, argsPtr, argc) => {
      const recv = target == null ? undefined : target[prop];
      const v = recv[method](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : Math.trunc(Number(v));
    },
    jsGetCallAsDouble: (target, prop, method, argsPtr, argc) => {
      const recv = target == null ? undefined : target[prop];
      const v = recv[method](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? 0 : Number(v);
    },
    jsGetCallAsBool: (target, prop, method, argsPtr, argc) => {
      const recv = target == null ? undefined : target[prop];
      return !!recv[method](...decodeJsSlots(getInstance(), argsPtr, argc));
    },
    jsGetCallAsString: (target, prop, method, argsPtr, argc) => {
      const recv = target == null ? undefined : target[prop];
      const v = recv[method](...decodeJsSlots(getInstance(), argsPtr, argc));
      return v == null ? "" : String(v);
    },
    // Awaiting a JS Promise: `target` (already deref'd) is the thenable. Returning it lets the async
    // import wrapper (`wrapAsyncImport`) settle it and register the resolved value as a `js` handle.
    jsAwait: (target) => target,
    // Unboxing: a `js` handle (already deref'd) -> a Dream primitive/string.
    jsAsInt: (target) => (target == null ? 0 : (Number(target) | 0)),
    jsAsLong: (target) => (target == null ? 0 : Math.trunc(Number(target))),
    jsAsDouble: (target) => (target == null ? 0 : Number(target)),
    jsAsBool: (target) => !!target,
    jsAsString: (target) => (target == null ? "" : String(target)),
    jsIsNull: (target) => target === null || target === undefined,
    jsRelease: (target) => getInstance().releaseValue(target),
    // Callback wrappers: a Dream `fun` handed to a JS API arrives already wrapped as a JS callable
    // (with stable per-funcref identity, so add/removeEventListener pair correctly); returning it
    // registers it as a `js` handle.
    jsFunc: (handler) => handler,
    jsFunc0: (handler) => handler,
    // Generalized: `index` is the raw funcref-table index (not pre-wrapped) and `arity` the number
    // of `js` parameters; wrap it as `fun(js, …): void` of that arity.
    jsFuncN: (index, arity) => {
      const n = arity > 0 ? arity : 0;
      const sig = `fun(${Array(n).fill("js").join(",")}):void`;
      return getInstance().callback(index, sig);
    },
    // HTTP helpers (see src/stdlib/net/http_client.dream). Each performs the whole request and resolves with
    // the full response as a single `Uint8Array` (marshaled to a Dream `char[]`): an ASCII head
    // ("<status>\n" + "Name: value\n" ... + blank line) followed by the raw body bytes. Bridged via
    // extern async. `httpRequest` takes a string body; `httpRequestBytes` takes a `char[]` body
    // (arriving as a JS byte array) so binary payloads cross without a UTF-8 round-trip.
    httpRequest: (url, method, headersJson, body, timeoutMs) =>
      httpDo(url, method, headersJson, body, timeoutMs),
    httpRequestBytes: (url, method, headersJson, body, timeoutMs) =>
      httpDo(url, method, headersJson, Uint8Array.from(body || []), timeoutMs),
    // Filesystem helpers (see src/stdlib/io/file.dream). Synchronous. They route through `fsBackend()`:
    // Node's real `node:fs`, or an in-memory virtual filesystem in the browser (see `memFs`). Text
    // variants marshal `string`; the byte variants marshal `char[]` directly (binary-safe, no string
    // round-trip) - the bytes are bulk-copied across the boundary.
    fileRead: (path) => new TextDecoder("utf-8").decode(fsBackend().readBytes(path)),
    // Byte counts and file sizes are `long` (wasm i64) on the Dream side, so these return BigInt.
    fileWrite: (path, content) => {
      const bytes = new TextEncoder().encode(content);
      fsBackend().write(path, bytes);
      return BigInt(bytes.length);
    },
    fileAppend: (path, content) => {
      const bytes = new TextEncoder().encode(content);
      fsBackend().append(path, bytes);
      return BigInt(bytes.length);
    },
    // Returns the file's bytes; marshalResult turns them into a Dream `byte[]` via writeArray.
    fileReadBytes: (path) => fsBackend().readBytes(path),
    // `data` arrives as a JS array of byte values (marshaled from `byte[]`).
    fileWriteBytes: (path, data) => {
      fsBackend().write(path, Uint8Array.from(data));
      return BigInt(data.length);
    },
    fileExists: (path) => fsBackend().exists(path),
    fileDelete: (path) => fsBackend().remove(path),
    fileSize: (path) => BigInt(fsBackend().size(path)),
    fileIsDir: (path) => fsBackend().isDir(path),
    dirList: (path) => fsBackend().list(path).join("\n"),
    dirCreate: (path) => {
      try { fsBackend().mkdir(path); return true; } catch (_) { return false; }
    },
    dirCreateAll: (path) => {
      try { fsBackend().mkdirAll(path); return true; } catch (_) { return false; }
    },
    // Streaming file handles (see system/io/file_handle.dream). `fileOpen` returns a positive handle
    // id or a negative errno-style code; reads/writes/seek operate on the live OS fd (Node) or a
    // cursor into the in-memory VFS (browser) without preloading the whole file into Dream memory.
    fileOpen: (path, mode) => fileOpenHost(path, mode),
    fileHandleRead: (id, n) => fileHandleReadHost(id, n),
    fileHandleWrite: (id, data) => fileHandleWriteHost(id, data),
    fileHandleSeek: (id, pos) => fileHandleSeekHost(id, pos),
    fileHandleClose: (id) => { fileHandleCloseHost(id); },
    // Cryptographic digests and CSPRNG (see system/crypto/*.dream). Node uses `node:crypto`;
    // browsers use `getRandomValues` for randomness and sync host digests (Web Crypto subtle is async).
    cryptoSha256: (data) => cryptoSha256Bytes(data),
    cryptoSha512: (data) => cryptoSha512Bytes(data),
    cryptoHmacSha256: (key, data) => cryptoHmacSha256Bytes(key, data),
    cryptoSecureRandomBytes: (n) => Array.from(csprngBytes(n > 0 ? n : 0)),
    cryptoSecureRandomFill: null, // bound via wrapInPlaceByteArrayFill in load()
    // Unicode helpers (see system/text/unicode.dream). Mirrored natively in
    // src/execution/host/text.rs.
    unicodeNormalize: (text, form) => {
      const forms = ["NFC", "NFD", "NFKC", "NFKD"];
      const f = forms[form] || "NFC";
      return String(text).normalize(f);
    },
    unicodeToLower: (text) => String(text).toLocaleLowerCase(),
    unicodeGraphemes: (text) => {
      const s = String(text);
      if (typeof Intl !== "undefined" && typeof Intl.Segmenter === "function") {
        const seg = new Intl.Segmenter(undefined, { granularity: "grapheme" });
        return Array.from(seg.segment(s), (part) => part.segment);
      }
      return Array.from(s);
    },
    // Wall-clock helpers (see src/stdlib/system/datetime.dream). Mirrored natively in
    // src/execution/host/datetime.rs so `DateTime` works the same under wasmtime, Node, and the
    // browser. `dateNowMillis` returns a `long` (BigInt). `Date.getTimezoneOffset()` returns
    // minutes *west* of UTC, the opposite sign convention from the Rust side's `chrono`-based
    // "minutes east of UTC", hence the negation.
    dateNowMillis: () => BigInt(Date.now()),
    dateLocalOffsetMinutes: (millis) => -new Date(Number(millis)).getTimezoneOffset(),
    // High-resolution monotonic timer (see src/stdlib/system/stopwatch.dream).
    timeNowNanos: () => {
      if (isNode) {
        return process.hrtime.bigint();
      } else {
        return BigInt(Math.floor(performance.now() * 1000000));
      }
    },
    // Wall-clock delay for `Time.delay` (see system/time.dream).
    delayMs: (ms) => new Promise((resolve) => setTimeout(resolve, Math.max(0, ms | 0))),
    // Console helpers (see src/stdlib/system/system.dream). Synchronous, mirroring
    // src/execution/host/console.rs. In Node, reads block on fd 0 via `fs.readSync`; there is no
    // synchronous stdin in a browser, so `readLine`/`readKey` fall back to `prompt()` there (and
    // return "" / 0 when unavailable, e.g. in a Worker).
    consoleReadLine: () => consoleReadLineSync(),
    consoleReadKey: () => consoleReadKeySync(),
    consoleExit: (code) => {
      if (isNode) process.exit(code);
      throw new Error(`System.exit(${code}): no process to exit in the browser`);
    },
    // Process / platform helpers (see src/stdlib/system/system.dream). Mirrored natively in
    // src/execution/host/process.rs.
    processPlatform: () => {
      if (isNode) return 1;
      if (typeof window !== "undefined" || typeof self !== "undefined") return 2;
      return 3;
    },
    processOsFamily: () => {
      if (!isNode) return 2;
      return process.platform === "win32" ? 1 : 0;
    },
    processArgs: () => (isNode ? process.argv.slice(2).join("\n") : ""),
    processExePath: () => (isNode ? (process.execPath || "") : ""),
    processEnvGet: (name) => {
      if (!isNode) return "";
      const v = process.env[name];
      return v === undefined ? "" : ("1" + v);
    },
    processEnvSet: (name, value) => {
      if (isNode) process.env[name] = value;
    },
    processCwd: () => (isNode ? process.cwd() : "/"),
    processSetCwd: (path) => {
      if (!isNode) return false;
      try { process.chdir(path); return true; } catch (_) { return false; }
    },
  };
}

/**
 * WebGPU host for `system.gpu`. Buffers/textures/surfaces are tracked by integer id; kernels come
 * from the sibling `.wgsl` + `abi.gpu.kernels` metadata attached via `attachGpuAbi`.
 */
function makeGpuHost(getInstance) {
  const buffers = new Map(); // id -> { gpuBuffer, nbytes, cpu }
  const shaders = new Map();
  const textures = new Map(); // id -> { texture, width, height, cpu }
  const surfaces = new Map(); // id -> { canvas, context, width, height, configured }
  const pipelineCache = new Map();
  let nextId = 1;
  let devicePromise = null;
  let device = null;
  let gpuAbi = null;
  let wgslSource = null;
  let blitPipeline = null;
  let blitSampler = null;
  let blitBindLayout = null;

  const ERR_UNAVAILABLE = 1;
  const ERR_TIMEOUT = 2;
  const ERR_VALIDATION = 3;
  const ERR_OTHER = 4;

  function classifyErr(err) {
    const msg = String(err && err.message ? err.message : err);
    if (/not available|no WebGPU|no WebGPU adapter/i.test(msg)) return ERR_UNAVAILABLE;
    if (/timed out|timeout/i.test(msg)) return ERR_TIMEOUT;
    if (/WGSL|validation|compile/i.test(msg)) return ERR_VALIDATION;
    return ERR_OTHER;
  }

  async function ensureDevice() {
    if (device) return device;
    if (!devicePromise) {
      devicePromise = (async () => {
        if (!globalThis.navigator?.gpu) {
          throw new Error("WebGPU is not available in this environment");
        }
        const adapter = await Promise.race([
          navigator.gpu.requestAdapter(),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error("WebGPU requestAdapter timed out")), 8000),
          ),
        ]);
        if (!adapter) throw new Error("no WebGPU adapter");
        device = await adapter.requestDevice();
        return device;
      })().catch((err) => {
        devicePromise = null;
        throw err;
      });
    }
    return devicePromise;
  }

  function attachFromAbi(abi, sourceHint) {
    gpuAbi = abi && abi.gpu ? abi.gpu : null;
    if (gpuAbi && typeof sourceHint === "string") {
      wgslSource = sourceHint.replace(/\.wasm$/, ".wgsl").replace(/\.abi\.json$/, ".wgsl");
    }
  }

  function toU8(data) {
    return data instanceof Uint8Array ? data : Uint8Array.from(data || []);
  }

  async function ensureBlit(dev) {
    if (blitPipeline) return;
    const code = `
struct VSOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f, };
@vertex fn vs(@builtin(vertex_index) vi: u32) -> VSOut {
  var positions = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  var uvs = array<vec2f, 3>(vec2f(0.0, 1.0), vec2f(2.0, 1.0), vec2f(0.0, -1.0));
  var o: VSOut;
  o.pos = vec4f(positions[vi], 0.0, 1.0);
  o.uv = uvs[vi];
  return o;
}
@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;
@fragment fn fs(i: VSOut) -> @location(0) vec4f {
  return textureSample(tex, samp, i.uv);
}`;
    const module = dev.createShaderModule({ code });
    blitBindLayout = dev.createBindGroupLayout({
      entries: [
        { binding: 0, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "filtering" } },
        { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: "float" } },
      ],
    });
    blitPipeline = await dev.createRenderPipelineAsync({
      layout: dev.createPipelineLayout({ bindGroupLayouts: [blitBindLayout] }),
      vertex: { module, entryPoint: "vs" },
      fragment: {
        module,
        entryPoint: "fs",
        targets: [{ format: navigator.gpu.getPreferredCanvasFormat() }],
      },
      primitive: { topology: "triangle-list" },
    });
    blitSampler = dev.createSampler({ magFilter: "linear", minFilter: "linear" });
  }

  let loadWgslText = async (url) => {
    if (!url) throw new Error("no .wgsl URL; compile with Dream to emit sibling .wgsl");
    if (typeof fetch === "function") {
      const res = await fetch(url);
      if (!res.ok) throw new Error(`failed to fetch ${url}`);
      return await res.text();
    }
    throw new Error("fetch unavailable for .wgsl");
  };

  async function syncBufferToCpu(dev, b) {
    if (!b.gpuBuffer) return;
    const staging = dev.createBuffer({
      size: b.nbytes,
      usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
    });
    const encoder = dev.createCommandEncoder();
    encoder.copyBufferToBuffer(b.gpuBuffer, 0, staging, 0, b.nbytes);
    dev.queue.submit([encoder.finish()]);
    await staging.mapAsync(GPUMapMode.READ);
    const copy = staging.getMappedRange().slice(0);
    staging.unmap();
    staging.destroy();
    b.cpu = new Uint8Array(copy);
  }

  const host = {
    __attachGpuAbi: attachFromAbi,

    gpuIsAvailable: () => !!(globalThis.navigator && globalThis.navigator.gpu),
    gpuReady: () => device != null,
    gpuTryInit: async () => {
      try {
        await ensureDevice();
        return 0;
      } catch (e) {
        console.error("Dream Gpu.try_init:", e);
        return classifyErr(e);
      }
    },
    gpuFrame: () =>
      new Promise((resolve) => {
        if (typeof requestAnimationFrame === "function") {
          requestAnimationFrame(() => resolve());
        } else {
          setTimeout(resolve, 16);
        }
      }),
    gpuTimestamp: async () => {
      if (typeof performance !== "undefined" && performance.now) {
        return BigInt(Math.floor(performance.now() * 1e6));
      }
      return BigInt(Date.now()) * 1000000n;
    },

    gpuBufferAllocBytes: (n) => {
      const id = nextId++;
      buffers.set(id, { gpuBuffer: null, nbytes: Math.max(0, n | 0), cpu: null });
      return id;
    },
    gpuBufferWriteBytes: (id, data) => {
      const b = buffers.get(id);
      if (!b) throw new Error(`unknown GpuBuffer ${id}`);
      const arr = toU8(data);
      b.cpu = arr;
      b.nbytes = arr.byteLength;
      b.gpuBuffer = null;
    },
    gpuBufferWriteBytesAt: (id, byteOffset, data) => {
      const b = buffers.get(id);
      if (!b) throw new Error(`unknown GpuBuffer ${id}`);
      const arr = toU8(data);
      const off = Math.max(0, byteOffset | 0);
      if (!(b.cpu instanceof Uint8Array) || b.cpu.byteLength < b.nbytes) {
        b.cpu = new Uint8Array(Math.max(b.nbytes, off + arr.byteLength));
      }
      if (off + arr.byteLength > b.cpu.byteLength) {
        const grown = new Uint8Array(off + arr.byteLength);
        grown.set(b.cpu);
        b.cpu = grown;
      }
      b.cpu.set(arr, off);
      b.nbytes = Math.max(b.nbytes, off + arr.byteLength);
      b.gpuBuffer = null;
    },
    gpuBufferReadBytes: async (id, n) => host.gpuBufferReadBytesAt(id, 0, n),
    gpuBufferReadBytesAt: async (id, byteOffset, n) => {
      const b = buffers.get(id);
      if (!b) throw new Error(`unknown GpuBuffer ${id}`);
      const nbytes = Math.max(0, n | 0);
      const off = Math.max(0, byteOffset | 0);
      if (b.gpuBuffer) {
        const dev = await ensureDevice();
        await syncBufferToCpu(dev, b);
      }
      if (!(b.cpu instanceof Uint8Array) && !b.cpu) {
        return Array(nbytes).fill(0);
      }
      const src = b.cpu instanceof Uint8Array ? b.cpu : new Uint8Array(b.cpu.buffer || []);
      const slice = src.slice(off, off + nbytes);
      if (slice.length >= nbytes) return Array.from(slice);
      const out = Array(nbytes).fill(0);
      for (let i = 0; i < slice.length; i++) out[i] = slice[i];
      return out;
    },

    gpuDispatch: async (kernel, bufferIds, ex, ey, ez, uniforms) => {
      try {
        const dev = await ensureDevice();
        const meta = (gpuAbi && gpuAbi.kernels || []).find((k) => k.name === kernel);
        if (!meta) throw new Error(`unknown @compute kernel '${kernel}'`);
        const code = (typeof meta.source === "string" && meta.source.length > 0)
          ? meta.source
          : await loadWgslText(wgslSource);
        let pipe = pipelineCache.get(kernel);
        if (!pipe) {
          const module = dev.createShaderModule({ code });
          if (typeof module.getCompilationInfo === "function") {
            const info = await module.getCompilationInfo();
            const errs = (info.messages || []).filter((m) => m.type === "error");
            if (errs.length) {
              throw new Error(`WGSL compile error in kernel '${kernel}':\n` +
                errs.map((m) => `${m.message} @${m.lineNum}:${m.linePos}`).join("\n"));
            }
          }
          const entries = (meta.bindings || []).map((b) => ({
            binding: b.binding,
            visibility: GPUShaderStage.COMPUTE,
            buffer: {
              type: b.kind === "uniform"
                ? "uniform"
                : (b.read_write ? "storage" : "read-only-storage"),
            },
          }));
          const seen = new Set();
          const unique = [];
          for (const e of entries) {
            if (seen.has(e.binding)) continue;
            seen.add(e.binding);
            unique.push(e);
          }
          const layout = dev.createBindGroupLayout({ entries: unique });
          const pipeline = await dev.createComputePipelineAsync({
            layout: dev.createPipelineLayout({ bindGroupLayouts: [layout] }),
            compute: { module, entryPoint: meta.entry },
          });
          pipe = { pipeline, layout, meta };
          pipelineCache.set(kernel, pipe);
        }
        const ids = bufferIds || [];
        const resources = [];
        const usedBindings = new Set();
        let storageIdx = 0;
        const extra = toU8(uniforms);
        for (const bind of meta.bindings || []) {
          if (usedBindings.has(bind.binding)) continue;
          usedBindings.add(bind.binding);
          if (bind.kind === "uniform") {
            const ubuf = dev.createBuffer({
              size: 256,
              usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
            });
            const bytes = new Uint8Array(256);
            const i32 = new Int32Array(bytes.buffer);
            i32[0] = ex | 0;
            i32[1] = ey | 0;
            i32[2] = ez | 0;
            if (extra.byteLength > 0) {
              bytes.set(extra.subarray(0, Math.min(extra.byteLength, 256 - 12)), 12);
            }
            dev.queue.writeBuffer(ubuf, 0, bytes);
            resources.push({ binding: bind.binding, resource: { buffer: ubuf } });
          } else {
            const id = ids[storageIdx++] | 0;
            const b = buffers.get(id);
            if (!b) throw new Error(`missing buffer id ${id} for binding ${bind.binding}`);
            if (!b.gpuBuffer) {
              b.gpuBuffer = dev.createBuffer({
                size: Math.max(4, b.nbytes),
                usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC,
              });
              if (b.cpu) {
                const bytes = b.cpu instanceof Uint8Array
                  ? b.cpu
                  : new Uint8Array(b.cpu.buffer, b.cpu.byteOffset, b.cpu.byteLength);
                dev.queue.writeBuffer(b.gpuBuffer, 0, bytes);
              }
            }
            resources.push({ binding: bind.binding, resource: { buffer: b.gpuBuffer } });
          }
        }
        const bg = dev.createBindGroup({ layout: pipe.layout, entries: resources });
        const wg = meta.workgroup || [64, 1, 1];
        const gx = Math.max(1, Math.ceil((ex | 0) / (wg[0] || 64)));
        const gy = Math.max(1, Math.ceil((ey | 0) / (wg[1] || 1)));
        const gz = Math.max(1, Math.ceil((ez | 0) / (wg[2] || 1)));
        const encoder = dev.createCommandEncoder();
        const pass = encoder.beginComputePass();
        pass.setPipeline(pipe.pipeline);
        pass.setBindGroup(0, bg);
        pass.dispatchWorkgroups(gx, gy, gz);
        pass.end();
        dev.queue.submit([encoder.finish()]);
        await dev.queue.onSubmittedWorkDone();
        return 0;
      } catch (e) {
        console.error("Dream gpuDispatch:", e);
        return classifyErr(e);
      }
    },

    gpuShaderFromWgsl: (source, entry) => {
      const id = nextId++;
      shaders.set(id, { source: String(source), entry: String(entry) });
      return id;
    },
    gpuDispatchShader: async (shaderId, bufferIds, wx, wy, wz) => {
      const s = shaders.get(shaderId);
      if (!s) return ERR_OTHER;
      const prev = wgslSource;
      const prevAbi = gpuAbi;
      wgslSource = null;
      gpuAbi = {
        kernels: [{
          name: `__raw_${shaderId}`,
          entry: s.entry,
          workgroup: [wx || 64, wy || 1, wz || 1],
          bindings: (bufferIds || []).map((_, i) => ({
            name: `b${i}`, binding: i, kind: "storage", type: "f32", read_write: true,
          })),
        }],
      };
      const inline = s.source;
      const oldLoad = loadWgslText;
      loadWgslText = async () => inline;
      try {
        return await host.gpuDispatch(
          `__raw_${shaderId}`, bufferIds, wx || 1, wy || 1, wz || 1, [],
        );
      } finally {
        loadWgslText = oldLoad;
        wgslSource = prev;
        gpuAbi = prevAbi;
      }
    },

    gpuTextureCreateRgba8: (width, height) => {
      const id = nextId++;
      const w = Math.max(1, width | 0);
      const h = Math.max(1, height | 0);
      textures.set(id, { texture: null, width: w, height: h, cpu: new Uint8Array(w * h * 4) });
      return id;
    },
    gpuTextureWriteRgba: async (id, pixels, x, y, w, h) => {
      try {
        const t = textures.get(id);
        if (!t) throw new Error(`unknown GpuTexture ${id}`);
        const px = Math.max(0, x | 0);
        const py = Math.max(0, y | 0);
        const pw = Math.max(0, w | 0);
        const ph = Math.max(0, h | 0);
        const src = toU8(pixels);
        for (let row = 0; row < ph; row++) {
          const dstOff = ((py + row) * t.width + px) * 4;
          const srcOff = row * pw * 4;
          t.cpu.set(src.subarray(srcOff, srcOff + pw * 4), dstOff);
        }
        const dev = await ensureDevice();
        if (!t.texture) {
          t.texture = dev.createTexture({
            size: [t.width, t.height],
            format: "rgba8unorm",
            usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC,
          });
        }
        dev.queue.writeTexture(
          { texture: t.texture, origin: [px, py] },
          src,
          { bytesPerRow: pw * 4 },
          [pw, ph],
        );
        return 0;
      } catch (e) {
        console.error("Dream gpuTextureWriteRgba:", e);
        return classifyErr(e);
      }
    },
    gpuTextureReadRgba: async (id) => {
      const t = textures.get(id);
      if (!t) throw new Error(`unknown GpuTexture ${id}`);
      return Array.from(t.cpu);
    },

    gpuSurfaceFromCanvas: (canvasId) => {
      if (typeof document === "undefined") return -1;
      const el = document.getElementById(String(canvasId)) || document.querySelector("canvas");
      if (!el || typeof el.getContext !== "function") return -1;
      const id = nextId++;
      surfaces.set(id, {
        canvas: el,
        context: null,
        width: el.width || 1,
        height: el.height || 1,
        configured: false,
        lastTexture: null,
      });
      return id;
    },
    gpuSurfaceConfigure: (id, width, height) => {
      const s = surfaces.get(id);
      if (!s) throw new Error(`unknown GpuSurface ${id}`);
      s.width = Math.max(1, width | 0);
      s.height = Math.max(1, height | 0);
      s.canvas.width = s.width;
      s.canvas.height = s.height;
      s.configured = false;
    },
    gpuSurfacePresent: async (id) => {
      // Present is implicit in blit for v1 (canvas context swap).
      return surfaces.has(id) ? 0 : ERR_OTHER;
    },
    gpuRenderBlit: async (surfaceId, textureId) => {
      try {
        const s = surfaces.get(surfaceId);
        const t = textures.get(textureId);
        if (!s || !t) throw new Error("blit: bad surface/texture id");
        const dev = await ensureDevice();
        await ensureBlit(dev);
        if (!t.texture) {
          t.texture = dev.createTexture({
            size: [t.width, t.height],
            format: "rgba8unorm",
            usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC,
          });
          if (t.cpu) {
            dev.queue.writeTexture(
              { texture: t.texture },
              t.cpu,
              { bytesPerRow: t.width * 4 },
              [t.width, t.height],
            );
          }
        }
        if (!s.context) {
          s.context = s.canvas.getContext("webgpu");
          if (!s.context) throw new Error("canvas webgpu context unavailable");
        }
        if (!s.configured) {
          s.context.configure({
            device: dev,
            format: navigator.gpu.getPreferredCanvasFormat(),
            alphaMode: "opaque",
          });
          s.configured = true;
        }
        const view = s.context.getCurrentTexture().createView();
        const bg = dev.createBindGroup({
          layout: blitBindLayout,
          entries: [
            { binding: 0, resource: blitSampler },
            { binding: 1, resource: t.texture.createView() },
          ],
        });
        const encoder = dev.createCommandEncoder();
        const pass = encoder.beginRenderPass({
          colorAttachments: [{
            view,
            clearValue: { r: 0, g: 0, b: 0, a: 1 },
            loadOp: "clear",
            storeOp: "store",
          }],
        });
        pass.setPipeline(blitPipeline);
        pass.setBindGroup(0, bg);
        pass.draw(3);
        pass.end();
        dev.queue.submit([encoder.finish()]);
        await dev.queue.onSubmittedWorkDone();
        return 0;
      } catch (e) {
        console.error("Dream gpuRenderBlit:", e);
        return classifyErr(e);
      }
    },
  };

  return host;
}

/** Blocks and returns one line from stdin (without the trailing newline), or "" if unavailable. */
function consoleReadLineSync() {
  if (isNode) {
    let line = "";
    const buf = Buffer.alloc(1);
    while (true) {
      let n;
      try { n = _nodeFs.readSync(0, buf, 0, 1, null); } catch (_) { break; } // EOF/EAGAIN
      if (n === 0) break;
      const ch = buf.toString("utf8", 0, 1);
      if (ch === "\n") break;
      if (ch !== "\r") line += ch;
    }
    return line;
  }
  if (typeof prompt === "function") return prompt("") || "";
  return "";
}

/**
 * Blocks and returns one character code from stdin, or 0 for EOF/no character. Node has no
 * synchronous raw (unbuffered) terminal mode, so this reads one byte from fd 0 - interactive
 * terminals still wait for Enter, same as `readLine`, but piped input reads a single byte as-is.
 */
function consoleReadKeySync() {
  if (isNode) {
    const buf = Buffer.alloc(1);
    try {
      const n = _nodeFs.readSync(0, buf, 0, 1, null);
      return n === 0 ? 0 : buf[0];
    } catch (_) {
      return 0;
    }
  }
  if (typeof prompt === "function") {
    const s = prompt("") || "";
    return s.length > 0 ? s.charCodeAt(0) : 0;
  }
  return 0;
}

// Node's `fs`, preloaded by `load()` (it's async; the file host functions are synchronous, so the
// module must already be in hand by the time Dream calls them). Stays null in a browser.
let _nodeFs = null;
// Node's `crypto`, preloaded the same way for `system.crypto` host functions.
let _nodeCrypto = null;

/** Fills `count` cryptographically secure random bytes (Uint8Array). */
function csprngBytes(count) {
  const n = count > 0 ? count : 0;
  const out = new Uint8Array(n);
  if (n === 0) return out;
  if (globalThis.crypto && typeof globalThis.crypto.getRandomValues === "function") {
    globalThis.crypto.getRandomValues(out);
    return out;
  }
  if (isNode && _nodeCrypto) {
    return Uint8Array.from(_nodeCrypto.randomBytes(n));
  }
  throw new Error("no CSPRNG available");
}

/**
 * Sync SHA-256 / SHA-512 / HMAC-SHA256 for the `system.crypto` host ABI. Node uses `node:crypto`;
 * browsers use `crypto.subtle` via one-shot digest (async-only) so a compact sync fallback is used
 * when `createHash` is unavailable.
 */
function cryptoSha256Bytes(data) {
  const bytes = Uint8Array.from(data || []);
  if (isNode && _nodeCrypto) {
    return Array.from(_nodeCrypto.createHash("sha256").update(bytes).digest());
  }
  return Array.from(browserSha256(bytes));
}

function cryptoSha512Bytes(data) {
  const bytes = Uint8Array.from(data || []);
  if (isNode && _nodeCrypto) {
    return Array.from(_nodeCrypto.createHash("sha512").update(bytes).digest());
  }
  return Array.from(browserSha512(bytes));
}

function cryptoHmacSha256Bytes(key, data) {
  const keyBytes = Uint8Array.from(key || []);
  const dataBytes = Uint8Array.from(data || []);
  if (isNode && _nodeCrypto) {
    return Array.from(_nodeCrypto.createHmac("sha256", keyBytes).update(dataBytes).digest());
  }
  return Array.from(browserHmacSha256(keyBytes, dataBytes));
}

// --- Browser sync digest fallbacks (Web Crypto `subtle` is async-only) -------------------------

function rotr(x, n) {
  return (x >>> n) | (x << (32 - n));
}

function browserSha256(msg) {
  const K = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ]);
  const l = msg.length;
  const withLen = new Uint8Array(((l + 9 + 63) & ~63));
  withLen.set(msg);
  withLen[l] = 0x80;
  const bitLen = BigInt(l) * 8n;
  const dv = new DataView(withLen.buffer);
  dv.setUint32(withLen.length - 4, Number(bitLen & 0xffffffffn), false);
  dv.setUint32(withLen.length - 8, Number(bitLen >> 32n), false);
  let h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a;
  let h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19;
  const w = new Uint32Array(64);
  for (let i = 0; i < withLen.length; i += 64) {
    for (let t = 0; t < 16; t++) {
      w[t] = dv.getUint32(i + t * 4, false);
    }
    for (let t = 16; t < 64; t++) {
      const s0 = rotr(w[t - 15], 7) ^ rotr(w[t - 15], 18) ^ (w[t - 15] >>> 3);
      const s1 = rotr(w[t - 2], 17) ^ rotr(w[t - 2], 19) ^ (w[t - 2] >>> 10);
      w[t] = (w[t - 16] + s0 + w[t - 7] + s1) >>> 0;
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, hh = h7;
    for (let t = 0; t < 64; t++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const t1 = (hh + S1 + ch + K[t] + w[t]) >>> 0;
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0;
      d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    h0 = (h0 + a) >>> 0; h1 = (h1 + b) >>> 0; h2 = (h2 + c) >>> 0; h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0; h5 = (h5 + f) >>> 0; h6 = (h6 + g) >>> 0; h7 = (h7 + hh) >>> 0;
  }
  const out = new Uint8Array(32);
  const outDv = new DataView(out.buffer);
  outDv.setUint32(0, h0, false); outDv.setUint32(4, h1, false);
  outDv.setUint32(8, h2, false); outDv.setUint32(12, h3, false);
  outDv.setUint32(16, h4, false); outDv.setUint32(20, h5, false);
  outDv.setUint32(24, h6, false); outDv.setUint32(28, h7, false);
  return out;
}

function browserHmacSha256(key, data) {
  const block = 64;
  let k = key;
  if (k.length > block) k = browserSha256(k);
  if (k.length < block) {
    const padded = new Uint8Array(block);
    padded.set(k);
    k = padded;
  }
  const oPad = new Uint8Array(block);
  const iPad = new Uint8Array(block);
  for (let i = 0; i < block; i++) {
    oPad[i] = k[i] ^ 0x5c;
    iPad[i] = k[i] ^ 0x36;
  }
  const inner = new Uint8Array(block + data.length);
  inner.set(iPad);
  inner.set(data, block);
  const outer = new Uint8Array(block + 32);
  outer.set(oPad);
  outer.set(browserSha256(inner), block);
  return browserSha256(outer);
}

function browserSha512(msg) {
  // SHA-512 of empty and short inputs is enough for parity; delegate to Node when available.
  // Minimal SHA-512 for browser parity with native host (FIPS 180-4).
  const K = [
    0x428a2f98d728ae22n, 0x7137449123ef65cdn, 0xb5c0fbcfec4d3b2fn, 0xe9b5dba58189dbbcn,
    0x3956c25bf348b538n, 0x59f111f1b605d019n, 0x923f82a4af194f9bn, 0xab1c5ed5da6d8118n,
    0xd807aa98a3030242n, 0x12835b0145706fben, 0x243185be4ee4b28cn, 0x550c7dc3d5ffb4e2n,
    0x72be5d74f27b896fn, 0x80deb1fe3b1696b1n, 0x9bdc06a725c71235n, 0xc19bf174cf692694n,
    0xe49b69c19ef14ad2n, 0xefbe4786384f25e3n, 0x0fc19dc68b8cd5b5n, 0x240ca1cc77ac9c65n,
    0x2de92c6f592b0275n, 0x4a7484aa6ea6e483n, 0x5cb0a9dcbd41fbd4n, 0x76f988da831153b5n,
    0x983e5152ee66dfabn, 0xa831c66d2db43210n, 0xb00327c898fb213fn, 0xbf597fc7beef0ee4n,
    0xc6e00bf33da88fc2n, 0xd5a79147930aa725n, 0x06ca6351e003826fn, 0x142929670a0e6e70n,
    0x27b70a8546d22ffcn, 0x2e1b21385c26c926n, 0x4d2c6dfc5ac42aedn, 0x53380d139d95b3dfn,
    0x650a73548baf63den, 0x766a0abb3c77b2a8n, 0x81c2c92e47edaee6n, 0x92722c851482353bn,
    0xa2bfe8a14cf10364n, 0xa81a664bbc423001n, 0xc24b8b70d0f89791n, 0xc76c51a30654be30n,
    0xd192e819d6ef5218n, 0xd69906245565a910n, 0xf40e35855771202an, 0x106aa07032bbd1b8n,
    0x19a4c116b8d2d0c8n, 0x1e376c085141ab53n, 0x2748774cdf8eeb99n, 0x34b0bcb5e19b48a8n,
    0x391c0cb3c5c95a63n, 0x4ed8aa4ae3418acbn, 0x5b9cca4f7763e373n, 0x682e6ff3d6b2b8a3n,
    0x748f82ee5defb2fcn, 0x78a5636f43172f60n, 0x84c87814a1f0ab72n, 0x8cc702081a6439ecn,
    0x90befffa23631e28n, 0xa4506cebde82bde9n, 0xbef9a3f7b2c67915n, 0xc67178f2e372532bn,
    0xca273eceea26619cn, 0xd186b8c721c0c207n, 0xeada7dd6cde0eb1en, 0xf57d4f7fee6ed178n,
    0x06f067aa72176fban, 0x0a637dc5a2c898a6n, 0x113f9804bef90daen, 0x1b710b35131c471bn,
    0x28db77f523047d84n, 0x32caab7b40c72493n, 0x3c9ebe0a15c9bebcn, 0x431d67c49c100d4cn,
    0x4cc5d4becb3e42b6n, 0x597f299cfc657e2an, 0x5fcb6fab3ad6faecn, 0x6c44198c4a475817n,
  ];
  const rotr64 = (x, n) => (x >> BigInt(n)) | (x << (64n - BigInt(n)));
  const l = msg.length;
  const withLen = new Uint8Array(((l + 17 + 127) & ~127));
  withLen.set(msg);
  withLen[l] = 0x80;
  const bitLen = BigInt(l) * 8n;
  const dv = new DataView(withLen.buffer);
  dv.setUint32(withLen.length - 4, Number(bitLen & 0xffffffffn), false);
  dv.setUint32(withLen.length - 8, Number((bitLen >> 32n) & 0xffffffffn), false);
  let h0 = 0x6a09e667f3bcc908n, h1 = 0xbb67ae8584caa73bn, h2 = 0x3c6ef372fe94f82bn, h3 = 0xa54ff53a5f1d36f1n;
  let h4 = 0x510e527fade682d1n, h5 = 0x9b05688c2b3e6c1fn, h6 = 0x1f83d9abfb41bd6bn, h7 = 0x5be0cd19137e2179n;
  const w = new BigUint64Array(80);
  for (let i = 0; i < withLen.length; i += 128) {
    for (let t = 0; t < 16; t++) {
      w[t] = dv.getBigUint64(i + t * 8, false);
    }
    for (let t = 16; t < 80; t++) {
      const s0 = rotr64(w[t - 15], 1) ^ rotr64(w[t - 15], 8) ^ (w[t - 15] >> 7n);
      const s1 = rotr64(w[t - 2], 19) ^ rotr64(w[t - 2], 61) ^ (w[t - 2] >> 6n);
      w[t] = (w[t - 16] + s0 + w[t - 7] + s1) & 0xffffffffffffffffn;
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, hh = h7;
    for (let t = 0; t < 80; t++) {
      const S1 = rotr64(e, 14) ^ rotr64(e, 18) ^ rotr64(e, 41);
      const ch = (e & f) ^ (~e & g);
      const t1 = (hh + S1 + ch + K[t] + w[t]) & 0xffffffffffffffffn;
      const S0 = rotr64(a, 28) ^ rotr64(a, 34) ^ rotr64(a, 39);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) & 0xffffffffffffffffn;
      hh = g; g = f; f = e; e = (d + t1) & 0xffffffffffffffffn;
      d = c; c = b; b = a; a = (t1 + t2) & 0xffffffffffffffffn;
    }
    h0 = (h0 + a) & 0xffffffffffffffffn; h1 = (h1 + b) & 0xffffffffffffffffn;
    h2 = (h2 + c) & 0xffffffffffffffffn; h3 = (h3 + d) & 0xffffffffffffffffn;
    h4 = (h4 + e) & 0xffffffffffffffffn; h5 = (h5 + f) & 0xffffffffffffffffn;
    h6 = (h6 + g) & 0xffffffffffffffffn; h7 = (h7 + hh) & 0xffffffffffffffffn;
  }
  const out = new Uint8Array(64);
  const outDv = new DataView(out.buffer);
  outDv.setBigUint64(0, h0, false); outDv.setBigUint64(8, h1, false);
  outDv.setBigUint64(16, h2, false); outDv.setBigUint64(24, h3, false);
  outDv.setBigUint64(32, h4, false); outDv.setBigUint64(40, h5, false);
  outDv.setBigUint64(48, h6, false); outDv.setBigUint64(56, h7, false);
  return out;
}

/**
 * In-memory virtual filesystem used when there is no real FS host (i.e. in the browser), mirroring
 * how a C/C++ -> WASM toolchain (Emscripten's MEMFS) gives you a working filesystem inside the page.
 * Files persist for the page session only. Paths are keys; directories are inferred from prefixes.
 */
const memFiles = new Map(); // path -> Uint8Array
const memDirs = new Set(); // explicit directory markers
const memFs = {
  readBytes(path) {
    const bytes = memFiles.get(path);
    if (!bytes) throw new Error(`ENOENT: no such file '${path}'`);
    return bytes;
  },
  write(path, bytes) {
    memFiles.set(path, Uint8Array.from(bytes));
  },
  append(path, bytes) {
    const prev = memFiles.get(path) || new Uint8Array(0);
    const next = new Uint8Array(prev.length + bytes.length);
    next.set(prev, 0);
    next.set(bytes, prev.length);
    memFiles.set(path, next);
  },
  exists(path) {
    return memFiles.has(path) || memDirs.has(path) || this.isDir(path);
  },
  remove(path) {
    memDirs.delete(path);
    return memFiles.delete(path);
  },
  size(path) {
    const bytes = memFiles.get(path);
    return bytes ? bytes.length : -1;
  },
  isDir(path) {
    if (memDirs.has(path)) return true;
    const prefix = path.endsWith("/") ? path : path + "/";
    for (const key of memFiles.keys()) {
      if (key.startsWith(prefix)) return true;
    }
    return false;
  },
  list(path) {
    const prefix = path === "" || path === "." ? "" : path.endsWith("/") ? path : path + "/";
    const names = new Set();
    for (const key of memFiles.keys()) {
      if (!key.startsWith(prefix)) continue;
      const rest = key.slice(prefix.length);
      const slash = rest.indexOf("/");
      names.add(slash === -1 ? rest : rest.slice(0, slash));
    }
    for (const dir of memDirs) {
      if (!dir.startsWith(prefix)) continue;
      const rest = dir.slice(prefix.length);
      if (!rest) continue;
      const slash = rest.indexOf("/");
      names.add(slash === -1 ? rest : rest.slice(0, slash));
    }
    return Array.from(names).sort();
  },
  mkdir(path) {
    if (memFiles.has(path) || memDirs.has(path)) throw new Error(`EEXIST: '${path}'`);
    memDirs.add(path);
  },
  mkdirAll(path) {
    const parts = path.split("/").filter(Boolean);
    let cur = path.startsWith("/") ? "" : "";
    for (const part of parts) {
      cur = cur === "" ? (path.startsWith("/") ? "/" + part : part) : cur + "/" + part;
      if (!memDirs.has(cur) && !memFiles.has(cur)) memDirs.add(cur);
    }
  },
};

// Real-filesystem backend over Node's `fs`, normalized to the same byte-oriented shape as `memFs`.
let _nodeFsBackend = null;
function nodeFsBackend() {
  if (_nodeFsBackend) return _nodeFsBackend;
  const fs = _nodeFs;
  _nodeFsBackend = {
    readBytes: (p) => new Uint8Array(fs.readFileSync(p)),
    write: (p, bytes) => fs.writeFileSync(p, Buffer.from(bytes)),
    append: (p, bytes) => fs.appendFileSync(p, Buffer.from(bytes)),
    exists: (p) => fs.existsSync(p),
    remove: (p) => { try { fs.rmSync(p); return true; } catch (_) { return false; } },
    size: (p) => { try { return Number(fs.statSync(p).size); } catch (_) { return -1; } },
    isDir: (p) => { try { return fs.statSync(p).isDirectory(); } catch (_) { return false; } },
    list: (p) => { try { return fs.readdirSync(p).sort(); } catch (_) { return []; } },
    mkdir: (p) => { fs.mkdirSync(p); },
    mkdirAll: (p) => { fs.mkdirSync(p, { recursive: true }); },
  };
  return _nodeFsBackend;
}

/** The active filesystem backend: Node's real `fs` when available, else the in-memory `memFs`. */
function fsBackend() {
  return _nodeFs ? nodeFsBackend() : memFs;
}

// --- streaming file handles (system/io/file_handle.dream) ------------------------------------

const OPEN_ENOENT = -1;
const OPEN_EACCES = -2;
const OPEN_EINVAL = -3;
const OPEN_EIO = -4;

/** Live open handles keyed by id (Node fd or in-memory VFS cursor). */
const fileHandles = new Map();
let nextFileHandleId = 1;

function mapNodeOpenMode(mode) {
  switch (mode) {
    case "r": return "r";
    case "w": return "w";
    case "a": return "a";
    case "r+": return "r+";
    case "w+": return "w+";
    case "a+": return "a+";
    default: return null;
  }
}

function mapOpenError(e) {
  if (!e || !e.code) return OPEN_EIO;
  if (e.code === "ENOENT") return OPEN_ENOENT;
  if (e.code === "EACCES" || e.code === "EPERM") return OPEN_EACCES;
  return OPEN_EIO;
}

function modeCreates(mode) {
  return mode === "w" || mode === "a" || mode === "w+" || mode === "a+";
}

function fileOpenHost(path, mode) {
  const nodeMode = mapNodeOpenMode(mode);
  if (!nodeMode) return OPEN_EINVAL;
  if (_nodeFs) {
    try {
      const fd = _nodeFs.openSync(path, nodeMode);
      const id = nextFileHandleId++;
      fileHandles.set(id, { kind: "node", fd, path, pos: 0 });
      return id;
    } catch (e) {
      return mapOpenError(e);
    }
  }
  // Browser VFS: open a cursor without copying file bytes into Dream memory up front.
  if (!memFiles.has(path) && !modeCreates(mode)) return OPEN_ENOENT;
  if (!memFiles.has(path)) memFiles.set(path, new Uint8Array(0));
  const id = nextFileHandleId++;
  fileHandles.set(id, { kind: "mem", path, pos: 0 });
  return id;
}

function fileHandleReadHost(id, n) {
  const h = fileHandles.get(id);
  if (!h) return [];
  const count = n > 0 ? n : 0;
  if (h.kind === "node") {
    const buf = Buffer.alloc(count);
    const bytesRead = _nodeFs.readSync(h.fd, buf, 0, count, h.pos);
    h.pos += bytesRead;
    return Array.from(buf.subarray(0, bytesRead));
  }
  const bytes = memFiles.get(h.path);
  if (!bytes) return [];
  const end = Math.min(h.pos + count, bytes.length);
  const slice = bytes.subarray(h.pos, end);
  h.pos = end;
  return Array.from(slice);
}

function fileHandleWriteHost(id, data) {
  const h = fileHandles.get(id);
  if (!h) return BigInt(-1);
  const bytes = Uint8Array.from(data || []);
  if (h.kind === "node") {
    try {
      const written = _nodeFs.writeSync(h.fd, bytes, 0, bytes.length, h.pos);
      h.pos += written;
      return BigInt(written);
    } catch (_) {
      return BigInt(-1);
    }
  }
  const prev = memFiles.get(h.path) || new Uint8Array(0);
  const before = prev.subarray(0, Math.min(h.pos, prev.length));
  const afterStart = h.pos + bytes.length;
  const after = prev.length > afterStart ? prev.subarray(afterStart) : new Uint8Array(0);
  const next = new Uint8Array(before.length + bytes.length + after.length);
  next.set(before, 0);
  next.set(bytes, before.length);
  next.set(after, before.length + bytes.length);
  memFiles.set(h.path, next);
  h.pos = before.length + bytes.length;
  return BigInt(bytes.length);
}

function fileHandleSeekHost(id, pos) {
  const h = fileHandles.get(id);
  if (!h) return -1;
  const target = Number(pos);
  if (target < 0) return -1;
  h.pos = target;
  return 0;
}

function fileHandleCloseHost(id) {
  const h = fileHandles.get(id);
  if (!h) return;
  if (h.kind === "node") {
    try { _nodeFs.closeSync(h.fd); } catch (_) { /* ignore */ }
  }
  fileHandles.delete(id);
}

/** Default `env` builtins every Dream module imports (mirrors src/.../wasm_runner.rs). */
function defaultEnv(getInstance, options) {
  const writeOut = options.stdout || ((s) => (typeof process !== "undefined" ? process.stdout.write(s) : console.log(s)));
  const writeLine = options.stdout
    ? (s) => options.stdout(s + "\n")
    : (s) => console.log(s);

  return {
    print_string: (ptr) => writeOut(getInstance().readString(ptr)),
    println: (ptr) => writeLine(getInstance().readString(ptr)),
    print_int: (v) => writeOut(String(v)),
    print_float: (v) => writeOut(String(v)),
    print_double: (v) => writeOut(String(v)),
    print_char: (v) => writeOut(String.fromCharCode(v)),
    sin: Math.sin,
    cos: Math.cos,
    tan: Math.tan,
    asin: Math.asin,
    acos: Math.acos,
    atan: Math.atan,
    atan2: Math.atan2,
    abs: Math.abs,
    sqrt: Math.sqrt,
    pow: Math.pow,
    floor: Math.floor,
    ceil: Math.ceil,
    round: Math.round,
  };
}

/** True when running under Node (vs. a browser), used to pick the byte-loading strategy. */
const isNode = typeof process !== "undefined" && !!(process.versions && process.versions.node);

/** Fetches `.wasm`/`.abi.json` bytes from a URL or local file path, in browser or Node. */
async function fetchBytes(source) {
  if (source instanceof ArrayBuffer) return new Uint8Array(source);
  if (source instanceof Uint8Array) return source;
  // In a browser, always go through `fetch` - a bare relative path like "app.wasm" is a valid
  // URL there and must not fall through to the Node-only `fs` branch.
  if (!isNode && typeof fetch === "function") {
    const res = await fetch(source);
    if (!res.ok) throw new Error(`failed to fetch ${source}: ${res.status}`);
    return new Uint8Array(await res.arrayBuffer());
  }
  // Node fallback.
  const { readFile } = await import("node:fs/promises");
  return new Uint8Array(await readFile(source));
}

async function loadAbi(abi) {
  if (!abi) return null;
  if (typeof abi === "object" && abi.externs) return abi; // already parsed
  const bytes = await fetchBytes(abi);
  return JSON.parse(new TextDecoder("utf-8").decode(bytes));
}

/**
 * Source of the worker bootstrap module. It imports this same `dream.js` (so the worker reuses
 * all the env/`Dream` import wiring, including nested workers) and, on `init`, instantiates the
 * same `.wasm` bytes importing the parent's shared `WebAssembly.Memory`. Thereafter each `msg` or
 * `dispatch` runs a `fun(string):string` body via `__workerInvoke` and posts the reply back.
 *
 * Browser workers use `self.onmessage` / `self.postMessage`; Node `worker_threads` workers use
 * `parentPort` instead — pass `node: true` for that dialect.
 */
function workerBootSource(dreamUrl, { node = false } = {}) {
  if (node) {
    return `import { parentPort } from 'node:worker_threads';
import * as Dream from ${JSON.stringify(dreamUrl)};
let inst = null;
parentPort.on('message', async (m) => {
  if (m.t === 'init') {
    inst = await Dream.load(m.bytes, { abi: m.abi, memory: m.memory });
    parentPort.postMessage({ t: 'ready' });
  } else if (m.t === 'msg') {
    parentPort.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'dispatch') {
    parentPort.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'term') {
    parentPort.close();
  }
});
`;
  }
  return `import * as Dream from ${JSON.stringify(dreamUrl)};
let inst = null;
self.onmessage = async (e) => {
  const m = e.data;
  if (m.t === 'init') {
    inst = await Dream.load(m.bytes, { abi: m.abi, memory: m.memory });
    self.postMessage({ t: 'ready' });
  } else if (m.t === 'msg') {
    self.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'dispatch') {
    self.postMessage({ t: 'reply', data: await inst.__workerInvoke(m.fnIdx, m.env, m.data) });
  } else if (m.t === 'term') {
    self.close();
  }
};
`;
}

/**
 * Builds the `Dream`-module worker host functions (`workerSpawn`/`workerPost`/`workerRecv`/
 * `workerTerminate`/`workerPoolSpawn`/`workerPoolDispatch`) behind `src/stdlib/core/webworker.dream`.
 * Each worker is a real browser `Worker` or Node `worker_threads.Worker` running a fresh instance
 * of the same module, importing the parent's shared `WebAssembly.Memory`.
 * `workerRecv`/`workerPoolDispatch` are `extern async`, so they return Promises bridged into
 * Dream's scheduler.
 */
function makeWorkerModule(wasmBytes, abi, getSharedMemory) {
  const reg = new Map();
  let nextId = 1;
  /** Lazily resolved Node `worker_threads.Worker` constructor (null until first Node spawn). */
  let NodeWorkerCtor = null;

  const postJob = (state, job) => {
    if (state.ready) state.worker.postMessage(job);
    else state.queued.push(job);
  };

  const attachHandlers = (worker, state) => {
    // Browser: `onmessage` event with `e.data`. Node worker_threads: `message` event with data directly.
    if (typeof worker.on === "function" && isNode) {
      worker.on("message", (m) => {
        if (m.t === "ready") {
          state.ready = true;
          for (const q of state.queued) state.worker.postMessage(q);
          state.queued = [];
        } else if (m.t === "reply") {
          if (state.pending.length > 0) state.pending.shift()(m.data);
          else state.replies.push(m.data);
        }
      });
    } else {
      worker.onmessage = (e) => {
        const m = e.data;
        if (m.t === "ready") {
          state.ready = true;
          for (const q of state.queued) state.worker.postMessage(q);
          state.queued = [];
        } else if (m.t === "reply") {
          if (state.pending.length > 0) state.pending.shift()(m.data);
          else state.replies.push(m.data);
        }
      };
    }
  };

  const spawnWorker = (fnIndex, env) => {
    const state = {
      worker: null,
      fnIndex,
      env,
      pending: [],
      replies: [],
      ready: false,
      queued: [],
      blobUrl: null,
    };

    const finishSpawn = (worker) => {
      state.worker = worker;
      attachHandlers(worker, state);
      worker.postMessage({
        t: "init",
        bytes: wasmBytes,
        abi,
        memory: getSharedMemory(),
      });
    };

    if (isNode) {
      // Spawn synchronously once the ctor is cached; first call kicks off an async import and
      // queues the init until it resolves (postJob already buffers until `ready`).
      const startNode = (NodeWorker) => {
        const worker = new NodeWorker(workerBootSource(import.meta.url, { node: true }), {
          eval: true,
        });
        // Allow the Node process to exit once Dream's async main has settled even if a worker
        // handle is still referenced briefly during teardown.
        if (typeof worker.unref === "function") worker.unref();
        finishSpawn(worker);
      };
      if (NodeWorkerCtor) {
        startNode(NodeWorkerCtor);
      } else {
        // Synchronous-looking spawn from Dream's POV: register the id immediately; jobs queue
        // until the worker posts `ready`. The dynamic import of worker_threads is one-shot.
        import("node:worker_threads").then(({ Worker }) => {
          NodeWorkerCtor = Worker;
          startNode(Worker);
        });
      }
    } else if (typeof Worker !== "undefined") {
      const url = URL.createObjectURL(
        new Blob([workerBootSource(import.meta.url)], { type: "text/javascript" }),
      );
      state.blobUrl = url;
      finishSpawn(new Worker(url, { type: "module" }));
    } else {
      throw new Error(
        "WebWorker requires a browser Worker or Node worker_threads; neither is available in this environment",
      );
    }

    const id = nextId++;
    reg.set(id, state);
    return id;
  };

  return {
    // `body` arrives marshaled as a JS callable wrapping the Dream funcref; recover its raw funcidx
    // and closure-env word (0 for a non-capturing body).
    workerSpawn: (body, env) =>
      spawnWorker(body && body.__dreamFuncIndex != null ? body.__dreamFuncIndex : body, env),
    workerPoolSpawn: () => spawnWorker(0, 0),
    workerPost: (id, msg) => {
      const s = reg.get(id);
      if (!s) return;
      postJob(s, { t: "msg", fnIdx: s.fnIndex, env: s.env, data: msg });
    },
    workerPoolDispatch: (id, fnIndex, env, msg) =>
      new Promise((resolve) => {
        const s = reg.get(id);
        if (!s) return resolve("");
        s.pending.push(resolve);
        postJob(s, { t: "dispatch", fnIdx: fnIndex, env, data: msg });
      }),
    // extern async: resolve with the next reply (or "" if the worker is gone).
    workerRecv: (id) =>
      new Promise((resolve) => {
        const s = reg.get(id);
        if (!s) return resolve("");
        if (s.replies.length > 0) resolve(s.replies.shift());
        else s.pending.push(resolve);
      }),
    workerTerminate: (id) => {
      const s = reg.get(id);
      if (!s) return;
      try {
        if (s.worker) {
          s.worker.postMessage({ t: "term" });
          s.worker.terminate();
        }
      } catch (_) {
        /* already gone */
      }
      if (s.blobUrl) {
        try {
          URL.revokeObjectURL(s.blobUrl);
        } catch (_) {
          /* ignore */
        }
      }
      for (const p of s.pending) p("");
      reg.delete(id);
    },
  };
}

// Fallback limits when the JS engine doesn't support type reflection on module imports (see
// `makeLinearMemory` below). Mirrors `crate::mir::abi::MAX_MEMORY_PAGES`; the initial guess only
// needs to cover the static-data/shadow-stack region every module reserves up front, since the
// allocator grows the real heap on demand via `memory.grow`.
const FALLBACK_INITIAL_MEMORY_PAGES = 64;
const FALLBACK_MAX_MEMORY_PAGES = 65536;

/**
 * Every Dream module imports its linear memory as `(import "env" "memory" (memory ... shared))`
 * rather than defining it, so a spawned `WebWorker` instance can share the exact same
 * `WebAssembly.Memory` (backed by a `SharedArrayBuffer`) as its parent. Reads the exact
 * min/max/shared limits the module requires off the compiled `WebAssembly.Module` via the
 * "JS API type reflection" proposal (`{module,name,kind,type}` entries from
 * `WebAssembly.Module.imports`), falling back to generous constants on engines that don't expose
 * `type` on import descriptors yet.
 */
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
 * @returns {Promise<DreamInstance>}
 */
export async function load(source, options = {}) {
  const wasmBytes = await fetchBytes(source);
  const abi = await loadAbi(options.abi);
  const wasmModule = await WebAssembly.compile(wasmBytes);

  // Preload Node's `fs` so the synchronous File/FileHandle host functions can use it. In a browser
  // this stays unavailable and only surfaces an error if a file API is actually called.
  if (isNode && !_nodeFs) {
    try { _nodeFs = await import("node:fs"); } catch (_) { /* leave unavailable */ }
  }
  if (isNode && !_nodeCrypto) {
    try { _nodeCrypto = await import("node:crypto"); } catch (_) { /* leave unavailable */ }
  }

  // Late-bound instance reference so import wrappers can marshal against live memory.
  let instance = null;
  const getInstance = () => {
    if (!instance) throw new Error("instance not ready");
    return instance;
  };

  // Build the import object: default env builtins first, then user-provided externs.
  const importObject = { env: defaultEnv(getInstance, options) };
  const sharedMemory = options.memory ?? makeLinearMemory(wasmModule);
  importObject.env.memory = sharedMemory;

  const userImports = options.imports || {};
  const sigByName = new Map();
  if (abi) for (const e of abi.externs) sigByName.set(e.name, e);

  // Built-in `Dream` host module (dynamic `js` / regex / fetch helpers). User-supplied imports still win.
  // The `WebWorker` host functions need the raw module bytes + abi so a worker can instantiate a
  // fresh copy of the same module in its own thread/heap.
  const builtinDream = {
    ...defaultDreamModule(getInstance),
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

  // 1. User-supplied implementations win, keyed by extern (Dream function) name.
  for (const name of Object.keys(userImports)) {
    const sig = sigByName.get(name);
    const module = sig ? sig.module : "env";
    const field = sig ? sig.field : name;
    (importObject[module] ||= {})[field] = wrapFor(userImports[name], sig);
  }

  // 2. Auto-bind any remaining externs to matching JS globals so built-in APIs need no glue
  //    (e.g. `alert`, `@js("console","log")`, `@js("Math","max")`). Unresolved imports get a
  //    thrower stub so instantiation still succeeds and the error only surfaces if called.
  if (abi) {
    for (const e of abi.externs) {
      const bucket = (importObject[e.module] ||= {});
      if (bucket[e.field]) continue; // already provided by the user
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
 * load a module and immediately invoke its `main`. The `.abi.json` path is
 * derived from the `.wasm` URL unless `options.abi` is given, so a whole page can be just:
 *
 *   import { run } from "./dream.js";
 *   await run("app.wasm", { imports: { ... } });
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
