import { isNode, getNodeCrypto } from "../platform.js";

/** Fills `count` cryptographically secure random bytes (Uint8Array). */
export function csprngBytes(count) {
  const n = count > 0 ? count : 0;
  const out = new Uint8Array(n);
  if (n === 0) return out;
  if (globalThis.crypto && typeof globalThis.crypto.getRandomValues === "function") {
    globalThis.crypto.getRandomValues(out);
    return out;
  }
  if (isNode && getNodeCrypto()) {
    return Uint8Array.from(getNodeCrypto().randomBytes(n));
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
  if (isNode && getNodeCrypto()) {
    return Array.from(getNodeCrypto().createHash("sha256").update(bytes).digest());
  }
  return Array.from(browserSha256(bytes));
}

function cryptoSha512Bytes(data) {
  const bytes = Uint8Array.from(data || []);
  if (isNode && getNodeCrypto()) {
    return Array.from(getNodeCrypto().createHash("sha512").update(bytes).digest());
  }
  return Array.from(browserSha512(bytes));
}

function cryptoHmacSha256Bytes(key, data) {
  const keyBytes = Uint8Array.from(key || []);
  const dataBytes = Uint8Array.from(data || []);
  if (isNode && getNodeCrypto()) {
    return Array.from(getNodeCrypto().createHmac("sha256", keyBytes).update(dataBytes).digest());
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
export function makeCryptoHost() {
  return {
    cryptoSha256: (data) => cryptoSha256Bytes(data),
    cryptoSha512: (data) => cryptoSha512Bytes(data),
    cryptoHmacSha256: (key, data) => cryptoHmacSha256Bytes(key, data),
    cryptoSecureRandomBytes: (n) => Array.from(csprngBytes(n > 0 ? n : 0)),
    cryptoSecureRandomFill: null,
  };
}
