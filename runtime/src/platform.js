/** True when running under Node (vs. a browser). */
export const isNode = typeof process !== "undefined" && !!(process.versions && process.versions.node);

let _nodeFs = null;
let _nodeCrypto = null;

export function setNodeFs(fs) { _nodeFs = fs; }
export function setNodeCrypto(crypto) { _nodeCrypto = crypto; }
export function getNodeFs() { return _nodeFs; }
export function getNodeCrypto() { return _nodeCrypto; }
