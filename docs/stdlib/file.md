# File I/O

**Package:** `system.io` — `import system.io;`

Fallible ops return `Result<_, IoError>` (`message()` / `code()` such as `ENOENT`). Whole-file `File.*` APIs are [`async`](../language/async.md). Streaming handles use a sync primary API with optional `*_async` wrappers (`close` is sync-only).

```dream
import system;
import system.io;
```

## Platform notes

| Runtime | Filesystem |
| --- | --- |
| Native (`dream run`) | Real on-disk filesystem |
| Node.js | Real on-disk filesystem |
| Browser | In-memory VFS for the page session |

## `File` — whole-file ops

#### `await File.write(path, content): Result<long, IoError>`

Creates or overwrites a file with UTF-8 text. Returns `Ok(bytes_written)` on success — use for small config files and generated output.

```dream
await File.write("notes.txt", "hello\n");
```

#### `await File.append(path, content): Result<long, IoError>`

Appends UTF-8 text to an existing file (or creates it). Prefer over read-modify-write when adding log lines or incremental output.

```dream
await File.append("notes.txt", "world\n");
```

#### `await File.read(path): Result<string, IoError>`

Reads the entire file as UTF-8 text. Returns `Err` if missing — use `File.exists` first when absence is expected.

```dream
let text = await File.read("notes.txt");
System.print(text.unwrap_or(""));
```

#### `await File.read_bytes(path): Result<byte[], IoError>`

Reads the entire file as raw bytes. Use for images, WASM, or any non-UTF-8 binary payload.

```dream
let bytes = await File.read_bytes("image.png");
```

#### `await File.write_bytes(path, data): Result<long, IoError>`

Writes a byte array, creating or overwriting the file. Pair with `read_bytes` for binary round-trips.

```dream
await File.write_bytes("copy.png", bytes.unwrap_or(Buffer.alloc<byte>(0)));
```

#### `await File.delete(path): Result<bool, IoError>`

Deletes a file if it exists. Handle `Err` for permission or path issues.

```dream
switch (await File.delete("notes.txt")) {
    Ok(_) => {},
    Err(e) => System.println(e.code()),
}
```

#### `await File.create_dir(path): Result<bool, IoError>`

Creates a single directory (parent must already exist). Use `create_dir_all` when intermediate folders may be missing.

```dream
await File.create_dir("outdir");
```

#### `await File.create_dir_all(path): Result<bool, IoError>`

Creates a path and every missing parent directory. Prefer for output trees like `a/b/c` in one call.

```dream
await File.create_dir_all("a/b/c");
```

#### `await File.list(path): string[]`

Returns entry names in a directory (empty array if not a directory). Does not recurse — walk subdirs yourself if needed.

```dream
let entries = await File.list(".");
System.println(entries.length);
```

#### `File.exists(path): bool` (sync)

Sync probe for whether a path exists. Cheap guard before read/delete without awaiting.

```dream
if (File.exists("notes.txt")) {
    System.println("present");
}
```

#### `File.size(path): Option<long>` (sync)

Returns file size in bytes, or `None` if absent. Use before allocating a buffer for large reads.

```dream
System.println(File.size("notes.txt").unwrap_or(0L - 1L));
```

#### `File.is_dir(path): bool` (sync)

Returns whether the path is a directory. Distinguish files from folders before `list` or traversal.

```dream
System.println(File.is_dir("."));
```

#### `File.open(path): Result<FileStream, IoError>` (sync)

#### `await File.open_async(path): Result<FileStream, IoError>`

Opens a text-oriented stream for incremental read/write. Prefer over whole-file `read` when the file is large or you process chunk by chunk.

```dream
let stream = File.open("notes.txt").unwrap_or(FileStream(FileHandle(0, "")));
stream.close();
```

## `FileHandle`

OS-backed read/write/seek stream. Modes: `"r"`, `"w"`, `"a"`, `"r+"`, `"w+"`, `"a+"`.

#### `FileHandle.open(path, mode): Result<FileHandle, IoError>`

#### `await FileHandle.open_async(path, mode): Result<FileHandle, IoError>`

Opens a low-level byte handle with an explicit mode. Use when you need seek/random access or binary I/O without the `FileStream` text cursor.

```dream
let handle = FileHandle.open("notes.txt", "r").unwrap_or(FileHandle(0, ""));
```

#### `read(n): Result<byte[], IoError>` / `await read_async(n)`

Reads up to `n` bytes from the current position. Returns fewer bytes at EOF — loop until empty or use `FileStream` for text.

```dream
let chunk = handle.read(16);
```

#### `write(data): Result<int, IoError>` / `await write_async(data)`

Writes raw bytes at the current position. Returns bytes written or an `IoError`.

```dream
let wh = FileHandle.open("out.bin", "w").unwrap_or(FileHandle(0, ""));
let data = Buffer.alloc<byte>(2);
data[0] = (byte)104;  // 'h'
data[1] = (byte)105;  // 'i'
wh.write(data);
```

#### `seek(pos: long): Result<bool, IoError>` / `await seek_async(pos)`

Seeks to an absolute byte offset from the file start. Use for random-access formats or re-reading a header.

```dream
handle.seek(0L);
```

#### `close(): void`

Releases the OS handle. Always close when done — streams do not auto-close on drop in all hosts.

```dream
handle.close();
```

Field: `path: string`.

## `FileStream`

Text-oriented cursor over a `FileHandle` (via `File.open`).

#### `read(n): string` / `read_bytes(n): byte[]` / `read_all(): string`

Reads the next `n` characters/bytes or the remainder of the file. `read_all` slurps everything left — fine for moderate files.

```dream
System.println(stream.read(5));
let head = stream.read_bytes(4);
let rest = stream.read_all();
```

#### `has_more(): bool` / `position(): int` / `.length`

Reports whether unread content remains, the current cursor offset, and total file size (`-1` if unknown). Drive chunked reads with `has_more`.

```dream
System.println(stream.position());
System.println(stream.length);  // total file size, or -1 if missing
while (stream.has_more()) {
    System.print(stream.read(16));
}
```

#### `seek(offset): void` / `reset(): void` / `close(): void`

Repositions the cursor (`reset` → start), then closes the underlying handle. Seek before re-parsing a file from the top.

```dream
stream.seek(0);
stream.reset();
stream.close();
```

## `Path`

Pure string path helpers — no filesystem access. Use to compose and inspect paths before passing them to `File`.

#### `Path.join(a, b): string`

Joins two path segments with the platform separator. Prefer over manual `"/"` concatenation for cross-platform code.

```dream
System.println(Path.join("dir", "file.txt"));
```

#### `Path.of(parts: string[]): string`

Joins many segments in order. Use when building paths from a dynamic list of components.

```dream
System.println(Path.of(["a", "b", "c.txt"]));
```

#### `Path.file_name(path): Option<string>`

Returns the final path component (file or directory name). `None` for empty or root-only paths.

```dream
System.println(Path.file_name("/tmp/a.txt").unwrap_or(""));
```

#### `Path.extension(path): Option<string>`

Returns the substring after the last `.` in the final component. `None` when there is no extension.

```dream
System.println(Path.extension("a.txt").unwrap_or(""));  // txt
```

#### `Path.parent(path): Option<string>`

Returns the directory portion of a path. `None` when there is no parent (e.g. a bare filename).

```dream
System.println(Path.parent("/tmp/a.txt").unwrap_or(""));
```

#### `Path.is_absolute(path): bool`

Returns whether the path is absolute on the current platform. Branch logic for config search paths vs relative assets.

```dream
System.println(Path.is_absolute("/tmp"));
```

#### `Path.normalize(path): string`

Collapses `.` and `..` segments without touching the filesystem. Use before comparing or displaying paths built from user input.

```dream
System.println(Path.normalize("a/./b/../c"));
```

#### `Path.separator(): string`

Returns the platform directory separator (`/` or `\`). Rarely needed — `join` handles separators for you.

```dream
System.println(Path.separator());
```

## `IoError`

Implements [`Error`](option-result.md).

```dream
let e = IoError.not_found("missing.txt");
System.println(e.message());
System.println(e.code());  // ENOENT
```

Factories: `not_found`, `permission_denied`, `other(path, msg)`, `exists`.

A runnable example: [`sample/interop/file_io.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/file_io.dream).
