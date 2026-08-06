import { decodeJsSlots } from "../marshal.js";

/** Dynamic `js` type bridges (@js Dream hosts). */
export function makeJsHost(getInstance) {
  return {
    jsGlobal: (name) => globalThis[name],
    jsGlobalThis: () => globalThis,
    jsObject: () => ({}),
    jsArray: () => [],
    jsString: (value) => value,
    jsInt: (value) => value,
    jsLong: (value) => value,
    jsDouble: (value) => value,
    jsBool: (value) => value,
    jsGetV: (target, name) => (target == null ? undefined : target[name]),
    jsSetV: (target, name, value) => { if (target != null) target[name] = value; },
    jsSetSlot: (target, name, argsPtr, argc) => {
      const [value] = decodeJsSlots(getInstance(), argsPtr, argc);
      if (target != null) target[name] = value;
    },
    jsCallV: (target, name, argsPtr, argc) =>
      target[name](...decodeJsSlots(getInstance(), argsPtr, argc)),
    jsInvokeV: (target, argsPtr, argc) =>
      target(...decodeJsSlots(getInstance(), argsPtr, argc)),
    jsGetCallV: (target, prop, method, argsPtr, argc) => {
      const recv = target == null ? undefined : target[prop];
      return recv[method](...decodeJsSlots(getInstance(), argsPtr, argc));
    },
    jsIndexGetV: (target, key) => (target == null ? undefined : target[key]),
    jsIndexSetV: (target, key, value) => { if (target != null) target[key] = value; },
    jsIndexSetSlot: (target, argsPtr, argc) => {
      const [key, value] = decodeJsSlots(getInstance(), argsPtr, argc);
      if (target != null) target[key] = value;
    },
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
    jsAwait: (target) => target,
    jsAsInt: (target) => (target == null ? 0 : (Number(target) | 0)),
    jsAsLong: (target) => (target == null ? 0 : Math.trunc(Number(target))),
    jsAsDouble: (target) => (target == null ? 0 : Number(target)),
    jsAsBool: (target) => !!target,
    jsAsString: (target) => (target == null ? "" : String(target)),
    jsIsNull: (target) => target === null || target === undefined,
    jsRelease: (target) => getInstance().releaseValue(target),
    jsFunc: (handler) => handler,
    jsFunc0: (handler) => handler,
    jsFuncN: (index, arity) => {
      const n = arity > 0 ? arity : 0;
      const sig = `fun(${Array(n).fill("js").join(",")}):void`;
      return getInstance().callback(index, sig);
    },
  };
}
