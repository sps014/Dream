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
  if (base === "JsRef") return inst.registerHandle(value);
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
  if (base === "JsRef") return inst.derefHandle(raw);
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
    // JS-object handle registry backing the Dream `JsRef` type. A `JsRef` crosses the boundary
    // as a small i32 id; the host keeps the real JS value here. Id 0 is reserved for null.
    this._jsHandles = new Map(); // id -> JS value
    this._jsIds = new Map(); // JS value -> id (identity for objects, value for primitives)
    this._jsNextId = 1;
    this._jsFreeIds = [];
  }

  /** Registers a JS value, returning its `JsRef` id (0 for null/undefined). Idempotent per value. */
  registerHandle(value) {
    if (value === null || value === undefined) return 0;
    const existing = this._jsIds.get(value);
    if (existing !== undefined) return existing;
    const id = this._jsFreeIds.length ? this._jsFreeIds.pop() : this._jsNextId++;
    this._jsHandles.set(id, value);
    this._jsIds.set(value, id);
    return id;
  }

  /** Resolves a `JsRef` id back to its JS value (null for id 0 / unknown). */
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
   * Reads a Dream string at `ptr` (a data pointer). Layout: `[len: i32][utf8...][\0]`, so the
   * length prefix gives the byte count directly; the trailing NUL is ignored.
   */
  readString(ptr) {
    if (!ptr) return "";
    const bytes = this.bytes;
    const len = this.view.getInt32(ptr, true);
    const start = ptr + 4;
    return new TextDecoder("utf-8").decode(bytes.subarray(start, start + len));
  }

  /**
   * Allocates a Dream string block for `str` and returns its data pointer, so JS-implemented
   * extern functions can return strings back into Dream. Requires the module to export `malloc`.
   * Layout: `[len: i32][utf8...][\0]`.
   */
  writeString(str) {
    if (typeof this.exports.malloc !== "function") {
      throw new Error("module does not export `malloc`; cannot allocate a string");
    }
    const encoded = new TextEncoder().encode(str);
    const ptr = this.exports.malloc(4 + encoded.length + 1, TAGS.STRING);
    const bytes = this.bytes;
    this.view.setInt32(ptr, encoded.length, true); // length prefix
    bytes.set(encoded, ptr + 4);
    bytes[ptr + 4 + encoded.length] = 0; // null terminator
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
    const fn = table.get(index);
    if (typeof fn !== "function") {
      throw new Error(`no Dream function at table index ${index}`);
    }
    const { params, result } = parseFunType(typeStr);
    return (...jsArgs) => {
      const raw = params.map((p, i) => jsToWasm(this, p, jsArgs[i]));
      const out = fn(...raw);
      return wasmToJs(this, result, out);
    };
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
    if (t === "JsRef") return inst.derefHandle(arg); // i32 handle id -> live JS value
    if (t.endsWith("[]")) return inst.readArray(arg, t.slice(0, -2));
    if (t === "bool") return arg !== 0;
    return arg; // numeric primitive or opaque pointer
  });
}

/** Marshals a JS return value back into the raw WASM value for the declared result type. */
function marshalResult(inst, result, ret) {
  if (result === "string") return inst.writeString(ret == null ? "" : String(ret));
  if (result === "bool") return ret ? 1 : 0;
  if (result === "JsRef") return inst.registerHandle(ret); // live JS value -> i32 handle id
  if (typeof result === "string" && result.endsWith("[]")) {
    return inst.writeArray(ret == null ? [] : ret, result.slice(0, -2)); // e.g. char[] file bytes
  }
  if (result === "void" || result == null) return ret == null ? 0 : ret;
  return ret;
}

/** Wraps a user-provided import implementation so its args/return are marshaled per the ABI. */
function wrapImport(getInstance, fn, signature) {
  const params = signature ? signature.params : null;
  const result = signature ? signature.result : null;

  return (...rawArgs) => {
    const inst = getInstance();
    const args = marshalArgs(inst, params, rawArgs);
    const ret = fn(...args);
    return marshalResult(inst, result, ret);
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
    Promise.resolve(fn(...args)).then((value) => {
      exports.__dream_resolve(future, marshalResult(inst, result, value));
      exports.__dream_run_loop();
    });
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
async function httpDo(url, method, headersJson, body) {
  const verb = (method || "GET").toUpperCase();
  const init = { method: verb };
  if (headersJson && headersJson !== "") {
    try { init.headers = JSON.parse(headersJson); } catch (_) { /* ignore bad header json */ }
  }
  const hasBody = typeof body === "string" ? body !== "" : body && body.length > 0;
  if (hasBody && verb !== "GET" && verb !== "HEAD") {
    init.body = body;
  }
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
}

/**
 * The built-in `Dream` host module backing the stdlib interop layer (`JsRef`, regex, fetch).
 * These run *after* argument marshaling, so a `JsRef` parameter arrives as the live JS value and
 * a `JsRef`/`string`/number result is marshaled back automatically. Only `jsRelease` needs the
 * instance, to drop the handle for the value it was given.
 */
function defaultDreamModule(getInstance) {
  const prop = (target, name) => (target == null ? undefined : target[name]);
  return {
    // Value/handle constructors.
    jsGlobal: (name) => globalThis[name],
    jsString: (value) => value,
    jsInt: (value) => value,
    jsDouble: (value) => value,
    jsBool: (value) => value,
    // Property reads (coerced to the requested primitive, or another handle).
    jsGetProp: (target, name) => prop(target, name),
    jsGetString: (target, name) => { const v = prop(target, name); return v == null ? "" : String(v); },
    jsGetInt: (target, name) => { const v = prop(target, name); return v == null ? 0 : (Number(v) | 0); },
    jsGetDouble: (target, name) => { const v = prop(target, name); return v == null ? 0 : Number(v); },
    jsGetBool: (target, name) => !!prop(target, name),
    jsSetProp: (target, name, value) => { if (target != null) target[name] = value; },
    // Method invocation with 0/1/2 reference arguments.
    jsCall0: (target, name) => target[name](),
    jsCall1: (target, name, a) => target[name](a),
    jsCall2: (target, name, a, b) => target[name](a, b),
    // Misc.
    jsToString: (target) => (target == null ? "null" : String(target)),
    jsIsNull: (target) => target === null || target === undefined,
    jsRelease: (target) => getInstance().releaseValue(target),
    // Invoke a JS function held as a JsRef (a JS -> Dream callback that Dream calls back).
    jsInvoke0: (fn) => fn(),
    jsInvoke1: (fn, a) => fn(a),
    jsInvoke2: (fn, a, b) => fn(a, b),
    // Regex helpers (string-in/string-out; see src/stdlib/text/regex.dream). Mirrored natively in
    // src/execution/host.rs so `Regex` works the same under wasmtime, Node, and the browser.
    regexTest: (pattern, flags, input) => new RegExp(pattern, flags).test(input),
    regexReplace: (pattern, flags, input, replacement) =>
      input.replace(new RegExp(pattern, flags), replacement),
    regexMatchJoined: (pattern, flags, input, sep) => {
      const m = input.match(new RegExp(pattern, flags));
      return m ? Array.from(m).join(sep) : "";
    },
    // HTTP helpers (see src/stdlib/net/http_client.dream). Each performs the whole request and resolves with
    // the full response as a single `Uint8Array` (marshaled to a Dream `char[]`): an ASCII head
    // ("<status>\n" + "Name: value\n" ... + blank line) followed by the raw body bytes. Bridged via
    // extern async. `httpRequest` takes a string body; `httpRequestBytes` takes a `char[]` body
    // (arriving as a JS byte array) so binary payloads cross without a UTF-8 round-trip.
    httpRequest: (url, method, headersJson, body) =>
      httpDo(url, method, headersJson, body),
    httpRequestBytes: (url, method, headersJson, body) =>
      httpDo(url, method, headersJson, Uint8Array.from(body || [])),
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
    // Wall-clock helpers (see src/stdlib/system/datetime.dream). Mirrored natively in
    // src/execution/host/datetime.rs so `DateTime` works the same under wasmtime, Node, and the
    // browser. `dateNowMillis` returns a `long` (BigInt). `Date.getTimezoneOffset()` returns
    // minutes *west* of UTC, the opposite sign convention from the Rust side's `chrono`-based
    // "minutes east of UTC", hence the negation.
    dateNowMillis: () => BigInt(Date.now()),
    dateLocalOffsetMinutes: (millis) => -new Date(Number(millis)).getTimezoneOffset(),
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
  };
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

/**
 * In-memory virtual filesystem used when there is no real FS host (i.e. in the browser), mirroring
 * how a C/C++ -> WASM toolchain (Emscripten's MEMFS) gives you a working filesystem inside the page.
 * Files persist for the page session only. Paths are keys; directories are inferred from prefixes.
 */
const memFiles = new Map(); // path -> Uint8Array
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
    return memFiles.has(path) || this.isDir(path);
  },
  remove(path) {
    return memFiles.delete(path);
  },
  size(path) {
    const bytes = memFiles.get(path);
    return bytes ? bytes.length : -1;
  },
  isDir(path) {
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
    return Array.from(names).sort();
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
  };
  return _nodeFsBackend;
}

/** The active filesystem backend: Node's real `fs` when available, else the in-memory `memFs`. */
function fsBackend() {
  return _nodeFs ? nodeFsBackend() : memFs;
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
    abs: Math.abs,
    sqrt: Math.sqrt,
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

  // Preload Node's `fs` so the synchronous File/FileStream host functions can use it. In a browser
  // this stays unavailable and only surfaces an error if a file API is actually called.
  if (isNode && !_nodeFs) {
    try { _nodeFs = await import("node:fs"); } catch (_) { /* leave unavailable */ }
  }

  // Late-bound instance reference so import wrappers can marshal against live memory.
  let instance = null;
  const getInstance = () => {
    if (!instance) throw new Error("instance not ready");
    return instance;
  };

  // Build the import object: default env builtins first, then user-provided externs.
  const importObject = { env: defaultEnv(getInstance, options) };

  const userImports = options.imports || {};
  const sigByName = new Map();
  if (abi) for (const e of abi.externs) sigByName.set(e.name, e);

  // Built-in `Dream` host module (JsRef / regex / fetch helpers). User-supplied imports still win.
  const builtinDream = defaultDreamModule(getInstance);

  const wrapFor = (fn, sig) =>
    sig && sig.async ? wrapAsyncImport(getInstance, fn, sig) : wrapImport(getInstance, fn, sig);

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

  const { instance: wasmInstance } = await WebAssembly.instantiate(wasmBytes, importObject);
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
