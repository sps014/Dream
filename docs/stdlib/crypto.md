# Crypto

**Package:** `system.crypto` — `import system.crypto;`

Host-backed digests and a cryptographically secure random byte generator. Hex and Base64 encoding stay in [`encoding`](encoding.md) — crypto APIs return raw `byte[]`.

## Platform notes

| Runtime | Digests / CSPRNG |
| --- | --- |
| Native (`dream run`) | OS CSPRNG and native digest libraries |
| Node.js | `node:crypto` |
| Browser | Web Crypto |

| Type | Member | Description |
| --- | --- | --- |
| `Sha256` | `hash(data)` | SHA-256 digest (32 bytes) |
| `Sha512` | `hash(data)` | SHA-512 digest (64 bytes) |
| `HmacSha256` | `sign(key, data)` | HMAC-SHA256 (32 bytes) |
| `SecureRandom` | `bytes(n)` | `n` random bytes (`n <= 0` → empty array) |
| `SecureRandom` | `fill(dest)` | overwrite `dest` in place |

Non-goals: TLS configuration, certificates, symmetric ciphers (AES-GCM).

```dream
import system;
import system.crypto;
import system.encoding;

fun main(): void {
    let msg = Encoding.utf8_encode("hello");
    System.println(Encoding.hex_encode(Sha256.hash(msg)));

    let key = Encoding.utf8_encode("secret");
    System.println(Encoding.hex_encode(HmacSha256.sign(key, msg)));

    let nonce = SecureRandom.bytes(16);
    System.println(Encoding.hex_encode(nonce));
}
```

For non-cryptographic PRNG (games, tests), use [`Random`](random.md) from `import system;`.
