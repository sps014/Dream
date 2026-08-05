# Logging

**Package:** `system.logging` — `import system.logging;`

Python-inspired levels, named loggers, and pluggable handlers. Not a full `dictConfig` clone — enough for apps and stdlib hosts.

## Levels

`LogLevel`: `Trace`, `Debug`, `Info`, `Warn`, `Error` (increasing severity). A logger drops records below its minimum level.

## Loggers and handlers

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

| Type | Role |
| --- | --- |
| `Logger.get(name)` | named singleton logger |
| `set_level` / `add_handler` | filter and fan-out |
| `trace` / `debug` / `info` / `warn` / `error` | emit |
| `ConsoleHandler` | `System.println` |
| `FileHandler(path)` | append whole lines via host `fileAppend` |
| `LogRecord` | `level`, `name`, `message`, `timestamp_ms` |
| `LogHandler` | `emit(record)` interface |

`Logger.get` reuses the same instance for a given name within the process.
