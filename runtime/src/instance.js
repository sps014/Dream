import { TAGS, elementSize, stripSuffix, parseFunType, jsToWasm, wasmToJs } from "./core.js";

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
