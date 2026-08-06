# File I/O

**Package:** `system.io` — `import system.io;`

Fallible ops return `Result<_, IoError>` (`message()` / `code()` such as `ENOENT`). `Path` helpers (`join`, `file_name`, `normalize`, …) live in the same package.

`File`, `FileHandle`, and `FileStream` are the filesystem API. Whole-file `File.read`/`write`/`…` are [`async`](../language/async.md). Streaming handles use a **sync primary API** (`open`/`read`/`write`/`seek`/`close`) with optional `*_async` wrappers (except `close`, which is sync-only). The same `.dream` runs unchanged on every host.

## Platform notes

| Runtime | Filesystem |
| --- | --- |
| Native (`dream run`) | Real on-disk filesystem |
| Node.js | Real on-disk filesystem |
| Browser | In-memory virtual filesystem; files persist for the page session only |

The API is identical everywhere; only the browser differs in that writes live in memory.

## Reading and writing text

`File.read` returns the whole file as a UTF-8 string; `File.write` replaces its contents and `File.append` adds to the end. Each resolves a `Future`, so `await` them in an `async fun`. Fallible operations resolve with a `Result`, so failure is explicit — read the value with `unwrap_or` or `switch`:

```dream
import system;
import system.io;

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
    let bytes = await File.read_bytes("image.png");   // Result<byte[], IoError>
    await File.write_bytes("copy.png", bytes.unwrap_or(Buffer.alloc<byte>(0)));
}
```

## Streaming with FileHandle and FileStream

`FileHandle.open` returns an OS-backed read/write/seek stream — bytes are read on demand, not preloaded into memory. Host IO is synchronous; use `*_async` when you want a `Future`. `close` is sync-only. `File.open` wraps a read-only `FileHandle` in a `FileStream` cursor for the familiar text-oriented API:

```dream
async fun main(): void {
    // Low-level handle: chunked reads and writes (sync primary API).
    let opened = FileHandle.open("notes.txt", "r");
    let handle = opened.unwrap_or(FileHandle(0, ""));

    let chunk = handle.read(16);   // Result<byte[], IoError>
    handle.seek(0L);
    handle.close();

    // Same operations via await-style wrappers.
    let opened2 = await FileHandle.open_async("notes.txt", "r");
    let handle2 = opened2.unwrap_or(FileHandle(0, ""));
    let chunk2 = await handle2.read_async(16);
    await handle2.seek_async(0L);
    handle2.close();

    // FileStream: text-oriented cursor over a live handle (via File.open).
    let stream_opened = File.open("notes.txt");
    let stream = stream_opened.unwrap_or(FileStream(FileHandle(0, "")));

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

Open modes for `FileHandle.open`: `"r"`, `"w"`, `"a"`, `"r+"`, `"w+"`, `"a+"`.

## API reference

### File

| Member | Description |
| --- | --- |
| `File.read(path): Future<Result<string, IoError>>` | whole file as UTF-8 text; `Err` if missing |
| `File.write(path, content): Future<Result<long, IoError>>` | overwrite; `Ok(bytes_written)` or `Err` |
| `File.append(path, content): Future<Result<long, IoError>>` | append; `Ok(bytes_written)` or `Err` |
| `File.read_bytes(path): Future<Result<byte[], IoError>>` | whole file as raw bytes; `Err` if missing |
| `File.write_bytes(path, data): Future<Result<long, IoError>>` | write raw bytes; `Ok(bytes_written)` or `Err` |
| `File.delete(path): Future<bool>` | delete; resolves `true` on success |
| `File.list(path): Future<string[]>` | directory entry names (empty if not a directory) |
| `File.exists(path): bool` | true if `path` exists (synchronous) |
| `File.size(path): Option<long>` | size in bytes, or `None` if missing (synchronous) |
| `File.is_dir(path): bool` | true if `path` is a directory (synchronous) |
| `File.open(path): Result<FileStream, IoError>` | open a seekable read stream over a live handle; `Err` if missing |
| `File.open_async(path): Future<Result<FileStream, IoError>>` | async wrapper around `open` |

### FileHandle

An OS-backed read/write/seek stream. Reads and writes are chunked — the whole file is not loaded into memory at open time. Primary methods are synchronous; `*_async` variants return a `Future`.

| Member | Description |
| --- | --- |
| `FileHandle.open(path, mode): Result<FileHandle, IoError>` | open with mode `"r"`, `"w"`, `"a"`, `"r+"`, `"w+"`, or `"a+"` |
| `open_async(path, mode): Future<Result<FileHandle, IoError>>` | async wrapper around `open` |
| `read(n): Result<byte[], IoError>` | read up to `n` bytes from the current position |
| `read_async(n): Future<Result<byte[], IoError>>` | async wrapper around `read` |
| `write(data): Result<int, IoError>` | write bytes at the current position; `Ok(bytes_written)` |
| `write_async(data): Future<Result<int, IoError>>` | async wrapper around `write` |
| `seek(pos): Result<bool, IoError>` | seek to absolute byte offset `pos` from the start |
| `seek_async(pos): Future<Result<bool, IoError>>` | async wrapper around `seek` |
| `close(): void` | release the OS handle (sync only) |

### FileStream

A seekable text-oriented cursor over an open `FileHandle`. `read`/`read_all` produce text; `read_bytes` produces a raw `byte[]`. The cursor advances on each read.

| Member | Description |
| --- | --- |
| `read(n): string` | up to `n` bytes from the cursor as text |
| `read_bytes(n): byte[]` | up to `n` raw bytes from the cursor |
| `read_all(): string` | everything remaining as text |
| `has_more(): bool` | true while the cursor has not reached end-of-file |
| `position(): int` | current cursor offset in bytes |
| `size(): int` | total file size in bytes (`-1` if missing) |
| `seek(offset): void` | move the cursor to an absolute offset from the start |
| `reset(): void` | rewind to the start |
| `close(): void` | release the underlying OS handle |

A runnable example lives in [`sample/interop/file_io.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/file_io.dream).
