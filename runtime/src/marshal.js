import { isFunType, stripSuffix } from "./core.js";

/** Marshals raw WASM argument values into JS values per the parameter type names. */
export function marshalArgs(inst, params, rawArgs) {
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
export function marshalResult(inst, result, ret) {
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
export function typeNeedsInstance(t) {
  if (!t || t === "void") return false;
  if (isFunType(t)) return true;
  const base = stripSuffix(t);
  return base === "string" || base === "js" || base.endsWith("[]");
}

/** True when any param/result of an extern needs heap marshaling via `getInstance()`. */
export function signatureNeedsInstance(params, result) {
  if (typeNeedsInstance(result)) return true;
  if (params) {
    for (const p of params) {
      if (typeNeedsInstance(p)) return true;
    }
  }
  return false;
}

/** Wraps a user-provided import implementation so its args/return are marshaled per the ABI. */
export function wrapImport(getInstance, fn, signature) {
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
export function wrapInPlaceByteArrayFill(getInstance, fillBytes) {
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
export const FUTURE_KIND_HOST = 1;
export const FUTURE_SLOTS_SIZE = 56; // F_SLOTS: a host future has no saved-locals region.

/**
 * Wraps an `extern async` import. The JS implementation returns a Promise; the wrapper
 * synchronously allocates a host `Future` and hands its pointer back to Dream, then resolves it
 * (and re-pumps the scheduler) once the Promise settles. This is the only place the JS `.then`
 * bridge lives - Dream source never sees a Promise.
 */
export function wrapAsyncImport(getInstance, fn, signature) {
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
export function resolveGlobal(module, field) {
  if (module === "env") {
    const g = globalThis[field];
    return typeof g === "function" ? g.bind(globalThis) : undefined;
  }
  const owner = globalThis[module];
  const fn = owner && owner[field];
  return typeof fn === "function" ? fn.bind(owner) : undefined;
}

// Slot tags for the dynamic-`js` argument buffer. One argument = one 16-byte slot laid out as
// `[tag: i32][aux: i32][payload: 8 bytes]`. Must match `src/mir/emit/types.rs::js_slot`.
export const JS_SLOT = {
  NULL: 0, INT: 1, LONG: 2, DOUBLE: 3, BOOL: 4, STRING: 5, JS: 6, FUNC: 7, ARRAY: 8,
};
// Maps an array element's slot tag (the `aux` word of an ARRAY slot) to the Dream element-type name
// understood by `readArray`.
export const JS_ARRAY_ELEM = {
  [JS_SLOT.INT]: "int", [JS_SLOT.LONG]: "long", [JS_SLOT.DOUBLE]: "double",
  [JS_SLOT.BOOL]: "bool", [JS_SLOT.STRING]: "string", [JS_SLOT.JS]: "js",
};

/**
 * Decodes `argc` tagged argument slots starting at `ptr` (in the instance's linear memory) into an
 * array of live JS values. Primitives are read in place, strings/arrays are materialized, `js`
 * handles are dereferenced, and Dream funcrefs are wrapped as identity-stable JS callables. This is
 * the read side of the shadow-stack marshaling emitted by `Emitter::emit_js_call`.
 */
export function decodeJsSlots(inst, ptr, argc) {
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
