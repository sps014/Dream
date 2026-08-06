# Encoding

**Package:** `system.encoding` — `import system.encoding;`

`Encoding` converts between Dream `string` / `byte[]` and common wire formats. Hex and Base64 helpers return `Result<_, ParseError>` on invalid input.

```dream
import system.encoding;

fun main(): void {
    let bytes = Encoding.utf8_encode("hi");
    let hex = Encoding.hex_encode(bytes);   // "6869"
}
```

| Member | Description |
| --- | --- |
| `Encoding.utf8_encode(text)` | UTF-8 bytes of `text` |
| `Encoding.utf8_decode(bytes)` | string from UTF-8 `bytes` |
| `Encoding.hex_encode(bytes)` | lowercase hex text |
| `Encoding.hex_decode(text)` | bytes from hex; `Err` on odd length / bad digits |
| `Encoding.base64_encode(bytes)` | standard Base64 with `=` padding |
| `Encoding.base64_decode(text)` | bytes from Base64; whitespace ignored |

Free helpers `hex_digit`, `hex_value`, and `b64_value` support codecs and are public for reuse.

```dream
import system;
import system.encoding;

fun main(): void {
    let bytes = Encoding.utf8_encode("hi");
    System.println(Encoding.hex_encode(bytes));
    switch (Encoding.base64_decode("aGk=")) {
        Ok(b) => System.println(Encoding.utf8_decode(b)),
        Err(e) => System.println(e.message()),
    }
}
```
