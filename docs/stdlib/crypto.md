# Crypto

**Package:** `system.crypto` — `import system.crypto;`

Host-backed digests and a cryptographically secure RNG. Hex/Base64 live in [`encoding`](encoding.md) — crypto APIs return raw `byte[]`.

## Platform notes

| Runtime | Digests / CSPRNG |
| --- | --- |
| Native (`dream run`) | OS CSPRNG and native digest libraries |
| Node.js | `node:crypto` |
| Browser | Web Crypto |

Non-goals: TLS, certificates, symmetric ciphers (AES-GCM).

```dream
import system;
import system.crypto;
import system.encoding;
```

#### `Sha256.hash(data: byte[]): byte[]`

Computes a 32-byte SHA-256 digest of `data`. Use for content fingerprints and as input to HMAC when a shorter hash is enough.

```dream
let msg = Encoding.utf8_encode("hello");
System.println(Encoding.hex_encode(Sha256.hash(msg)));
```

#### `Sha512.hash(data: byte[]): byte[]`

Computes a 64-byte SHA-512 digest. Prefer when you want a wider hash for long-term integrity or protocol requirements specify SHA-512.

```dream
System.println(Encoding.hex_encode(Sha512.hash(msg)));
```

#### `HmacSha256.sign(key: byte[], data: byte[]): byte[]`

Computes a 32-byte HMAC-SHA256 MAC with `key` over `data`. Use to authenticate messages or API payloads with a shared secret.

```dream
let key = Encoding.utf8_encode("secret");
System.println(Encoding.hex_encode(HmacSha256.sign(key, msg)));
```

#### `SecureRandom.bytes(n: int): byte[]`

Fills a new array with `n` cryptographically secure random bytes (`n <= 0` → empty). Use for tokens, nonces, and IVs — not for gameplay RNG.

```dream
let nonce = SecureRandom.bytes(16);
System.println(Encoding.hex_encode(nonce));
```

#### `SecureRandom.fill(dest: byte[]): void`

Overwrites an existing buffer with secure random bytes in place. Prefer when reusing a fixed-size buffer to avoid allocation.

```dream
let buf = Buffer.alloc<byte>(32);
SecureRandom.fill(buf);
```

For non-cryptographic PRNG, use [`Random`](random.md).
