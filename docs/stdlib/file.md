# File I/O

`File` and `FileStream` are the filesystem API. Operations are [`async`](../language/async.md) and return a `Future<T>` you `await`; the same `.dream` runs unchanged on every host.

## Runtime support

| Runtime | Filesystem |
| --- | --- |
| Wasmtime (native CLI) | Real on-disk filesystem |
| Node.js | Real on-disk filesystem via `node:fs` |
| Browser | In-memory virtual filesystem; files persist for the page session only |

The API is identical everywhere; only the browser differs in that writes live in memory.

## Reading and writing text

`File.read` returns the whole file as a UTF-8 string; `File.write` replaces its contents and `File.append` adds to the end. Each resolves a `Future`, so `await` them in an `async fun`. Fallible operations resolve with a `Result`, so failure is explicit — read the value with `unwrap_or` or `switch`:

```dream
async fun main(): void {
    await File.write("notes.txt", "hello\n");
    await File.append("notes.txt", "world\n");

    let text = await File.read("notes.txt");
    System.print(text.unwrap_or(""));    // "hello\nworld\n"
}
```

`read` and `read_bytes` resolve with `Err` when the file is missing; `write`, `append`, and `write_bytes` resolve with `Ok(bytes_written)` or `Err`.

## Metadata and directories

`exists`, `size`, and `is_dir` are cheap and synchronous — no `await`. `size` is an `Option<long>` (`None` if missing). `File.list` resolves to a `string[]` of entry names:

```dream
async fun main(): void {
    if (File.exists("notes.txt")) {
        System.println(File.size("notes.txt").unwrap_or(0L - 1L));   // bytes; -1 if missing
    }
    let entries = await File.list(".");
    System.println(entries.size());
}
```

## Binary I/O

For non-text data, `read_bytes`/`write_bytes` move raw bytes between the file and a `byte[]` in a single bulk copy — binary-safe (bytes such as `0x00` are preserved). Byte counts and sizes are `long`:

```dream
async fun main(): void {
    let bytes = await File.read_bytes("image.png");   // Result<byte[], string>
    await File.write_bytes("copy.png", bytes.unwrap_or(Buffer.alloc<byte>(0)));
}
```

## Streaming with FileStream

`File.open` reads a file's bytes once into a buffered, seekable cursor. Reads slice the buffer in place, text views are materialized only when asked for, and `seek` is allocation-free:

```dream
async fun main(): void {
    let opened = await File.open("notes.txt");           // Result<FileStream, string>
    let stream = opened.unwrap_or(FileStream(Buffer.alloc<byte>(0)));

    System.println(stream.read(5));        // first 5 bytes as text
    System.println(stream.position());     // 5

    stream.seek(0);                        // rewind
    let head = stream.read_bytes(4);       // first 4 bytes as byte[]

    while (stream.has_more()) {
        System.print(stream.read(16));     // 16-byte text chunks
    }
    stream.close();
}
```

## API reference

### File

| Member | Description |
| --- | --- |
| `File.read(path): Future<Result<string, string>>` | whole file as UTF-8 text; `Err` if missing |
| `File.write(path, content): Future<Result<long, string>>` | overwrite; `Ok(bytes_written)` or `Err` |
| `File.append(path, content): Future<Result<long, string>>` | append; `Ok(bytes_written)` or `Err` |
| `File.read_bytes(path): Future<Result<byte[], string>>` | whole file as raw bytes; `Err` if missing |
| `File.write_bytes(path, data): Future<Result<long, string>>` | write raw bytes; `Ok(bytes_written)` or `Err` |
| `File.delete(path): Future<bool>` | delete; resolves `true` on success |
| `File.list(path): Future<string[]>` | directory entry names (empty if not a directory) |
| `File.exists(path): bool` | true if `path` exists (synchronous) |
| `File.size(path): Option<long>` | size in bytes, or `None` if missing (synchronous) |
| `File.is_dir(path): bool` | true if `path` is a directory (synchronous) |
| `File.open(path): Future<Result<FileStream, string>>` | open a buffered read stream; `Err` if missing |

### FileStream

A buffered cursor over a file's bytes. `read`/`read_all` produce text; `read_bytes` produces a raw `byte[]`. The cursor advances on each read.

| Member | Description |
| --- | --- |
| `read(n): string` | up to `n` bytes from the cursor as text |
| `read_bytes(n): byte[]` | up to `n` raw bytes from the cursor |
| `read_all(): string` | everything remaining as text |
| `has_more(): bool` | true while unread bytes remain |
| `position(): int` | current cursor offset in bytes |
| `size(): int` | total number of buffered bytes |
| `seek(offset): void` | move the cursor to an absolute offset (clamped to `[0, size]`) |
| `reset(): void` | rewind to the start |
| `close(): void` | release the buffer (advances the cursor to the end) |

A runnable example lives in [`sample/interop/file_io.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/file_io.dream).
