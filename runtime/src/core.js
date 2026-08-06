// Runtime type tags and marshaling helpers (heap layout mirrors object.rs).

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

export const HEAP_HEADER_SIZE = 12;

export function elementSize(typeName) {
  if (typeName === "bool" || typeName === "char" || typeName === "byte") return 1;
  if (typeName === "double" || typeName === "long" || typeName === "ulong") return 8;
  return 4;
}

export function stripSuffix(typeName) {
  let t = typeName;
  if (t.endsWith("?")) t = t.slice(0, -1);
  return t;
}

export const isPrimitive = (t) => t === "int" || t === "float" || t === "double" || t === "bool";

export const isFunType = (t) => typeof t === "string" && t.startsWith("fun(");

export function parseFunType(typeStr) {
  const open = typeStr.indexOf("(");
  const close = typeStr.lastIndexOf(")");
  const inner = typeStr.slice(open + 1, close).trim();
  const result = typeStr.slice(close + 1).replace(/^:/, "").trim() || "void";
  const params = inner.length ? inner.split(",").map((s) => s.trim()) : [];
  return { params, result };
}

export function jsToWasm(inst, t, value) {
  const base = stripSuffix(t);
  if (base === "string") return inst.writeString(value == null ? "" : String(value));
  if (base === "bool") return value ? 1 : 0;
  if (base === "js") return inst.registerHandle(value);
  if (base === "void") return 0;
  if (base === "long" || base === "ulong") return BigInt(value == null ? 0 : value);
  return value == null ? 0 : value;
}

export function wasmToJs(inst, t, raw) {
  const base = stripSuffix(t);
  if (base === "string") return inst.readString(raw);
  if (base === "bool") return raw !== 0;
  if (base === "js") return inst.derefHandle(raw);
  if (base === "void") return undefined;
  return raw;
}
