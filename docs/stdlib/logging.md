# Logging

**Package:** `system.logging` — `import system.logging;`

Named loggers, levels, and pluggable handlers.

```dream
import system;
import system.logging;

fun main(): void {
    let log = Logger.get("app");
    log.add_handler(ConsoleHandler());
    log.set_level(LogLevel.Debug);
    log.info("ready");
}
```

## Levels

`LogLevel`: `Trace`, `Debug`, `Info`, `Warn`, `Error` (increasing severity). A logger drops records below its minimum level.

#### `level_name(level: LogLevel): string`

Returns the display name for a log level (e.g. `"Info"`). Use when formatting custom handlers or building level filters in config UI.

```dream
System.println(level_name(LogLevel.Info));  // "Info"
```

## `Logger`

#### `Logger.get(name: string): Logger`

Returns a named singleton logger for the process. Prefer over `Logger(name)` when multiple modules should share the same logger and handlers.

```dream
let log = Logger.get("app");
```

#### `Logger(name: string)`

Constructs a standalone logger without registering in the global table. Use for one-off tests or isolated pipelines that should not share handlers.

```dream
let private_log = Logger("one-shot");
```

#### `set_level(level: LogLevel): void`

Sets the minimum level — records below it are dropped. Tune per environment (e.g. `Debug` in dev, `Warn` in production).

```dream
log.set_level(LogLevel.Warn);
```

#### `add_handler(handler: LogHandler): void`

Attaches a sink that receives every record passing the level filter. Stack multiple handlers (console + file) on one logger.

```dream
log.add_handler(ConsoleHandler());
log.add_handler(FileHandler("app.log"));
```

#### `trace` / `debug` / `info` / `warn` / `error` `(msg: string): void`

Emits a record at the matching severity. Pick the lowest level that still conveys useful signal — avoid `error` for expected conditions.

```dream
log.trace("detail");
log.debug("probe");
log.info("ready");
log.warn("slow");
log.error("failed");
```

## Handlers

#### `ConsoleHandler()`

Writes each record to stdout via `System.println`. Default choice for development and CLI tools.

```dream
log.add_handler(ConsoleHandler());
```

#### `FileHandler(path: string)`

Appends one line per record to a file (host `fileAppend`). Use for persistent logs without building a custom handler.

```dream
log.add_handler(FileHandler("app.log"));
```

#### `LogHandler.emit(record: LogRecord): void`

Interface for custom sinks — implement `emit` to forward records to syslog, structured JSON, or remote aggregators.

## `LogRecord`

Fields: `level`, `name`, `message`, `timestamp_ms`.

```dream
let rec = LogRecord(LogLevel.Info, "app", "hi", DateTime.utc_now().epoch_millis);
System.println(rec.message);
```
