# Encoding

**Package:** `system.encoding` — `import system.encoding;`

Converts between Dream `string` / `byte[]` and common wire formats. Hex and Base64 decode return `Result<_, ParseError>` on invalid input.

```dream
import system;
import system.encoding;
```

#### `Encoding.utf8_encode(text: string): byte[]`

Encodes a string as UTF-8 bytes. Use before hashing, crypto, or binary wire formats that need raw octets.

```dream
let bytes = Encoding.utf8_encode("hi");
```

#### `Encoding.utf8_decode(bytes: byte[]): string`

Decodes UTF-8 bytes into a Dream string. Pair with `utf8_encode` for text round-trips through byte buffers.

```dream
System.println(Encoding.utf8_decode(bytes));  // hi
```

#### `Encoding.hex_encode(bytes: byte[]): string`

Encodes bytes as lowercase hexadecimal. Use for displaying digests, keys, and debug dumps of binary data.

```dream
System.println(Encoding.hex_encode(bytes));  // 6869
```

#### `Encoding.hex_decode(text: string): Result<byte[], ParseError>`

Parses a hex string into bytes. Returns `Err` on odd length or invalid digits — validate user input before use.

```dream
switch (Encoding.hex_decode("6869")) {
    Ok(b) => System.println(Encoding.utf8_decode(b)),
    Err(e) => System.println(e.message()),
}
```

`Err` on odd length or bad digits.

#### `Encoding.base64_encode(bytes: byte[]): string`

Encodes bytes as standard Base64 with `=` padding. Use for embedding binary in JSON, URLs, or text protocols.

```dream
System.println(Encoding.base64_encode(bytes));  // aGk=
```

#### `Encoding.base64_decode(text: string): Result<byte[], ParseError>`

Decodes Base64 text into bytes (whitespace ignored). Returns `Err` on illegal characters or bad padding.

```dream
switch (Encoding.base64_decode("aGk=")) {
    Ok(b) => System.println(Encoding.utf8_decode(b)),
    Err(e) => System.println(e.message()),
}
```

### Free helpers

#### `hex_digit(v: int): char` / `hex_value(c: char): int` / `b64_value(c: char): int`

Low-level nibble and Base64 digit converters exposed for custom codecs. Rarely needed when the `Encoding.*` helpers suffice.

```dream
System.println(hex_digit(15));           // 'f'
System.println(hex_value('a'));          // 10
System.println(b64_value('A'));          // 0
```
