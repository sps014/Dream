# Built-ins

**Packages:** bootstrap (`system.core` / `system.primitives` — no import) · console & probes via `import system;`

Bootstrap types (`Option`, `Result`, `Buffer`, `Math`, …) need no import. Console I/O and most higher-level APIs need an explicit import — see [Imports](../language/imports.md#standard-library-packages).

```dream
import system;
```

## Console output

#### `System.print<T>(value): void`

Writes any value with no trailing newline.

```dream
System.print(42);  // "42"
```

#### `System.println<T>(value): void`

Same, plus a newline. Classes with `@override to_string` are handled automatically.

```dream
System.println("hello");
System.println(true);
```

#### `System.print_colored(text: string, color: ConsoleColor): void`

Prints one string in color and resets (no newline).

```dream
System.print_colored("warning", ConsoleColor.Yellow);
```

#### `System.set_foreground(color)` / `System.set_background(color)` / `System.reset_color()`

Change color of subsequent output until reset.

```dream
System.set_foreground(ConsoleColor.Green);
System.set_background(ConsoleColor.Black);
System.println("ok");
System.reset_color();
```

#### `System.clear(): void`

Clears the terminal.

```dream
System.clear();
```

`ConsoleColor`: `Black`, `DarkBlue`, `DarkGreen`, `DarkCyan`, `DarkRed`, `DarkMagenta`, `DarkYellow`, `Gray`, `DarkGray`, `Blue`, `Green`, `Cyan`, `Red`, `Magenta`, `Yellow`, `White`.

These use ANSI escapes (macOS/Linux terminals and Windows 10+ with VT enabled).

## Console input

#### `System.read_line(): string`

Blocks until a full line; returns it without the trailing newline.

```dream
let line = System.read_line();
```

#### `System.read_key(): char`

Single keypress (no Enter, no echo). Keys with no character (e.g. arrows) yield `(char)0`. Browser / non-interactive stdin falls back to one byte.

```dream
let c = System.read_key();
```

#### `System.read_int(): Result<int, ParseError>`

#### `System.read_double(): Result<double, ParseError>`

#### `System.read_bool(): Result<bool, ParseError>`

```dream
System.print("age? ");
switch (System.read_int()) {
    Ok(v)  => System.println("age: " + v.to_string()),
    Err(e) => System.println("invalid input: " + e.message()),
}
```

#### `System.exit(code: int): void`

Terminates immediately; never returns.

```dream
System.exit(0);
```

#### `System.panic(message: string): void`

Fatal halt after printing `message` — see [Panics](../language/panics.md).

```dream
System.panic("unreachable");
```

## Platform, env, and process

#### `System.platform(): Platform`

`Native`, `Node`, `Browser`, `Unknown`.

```dream
System.println(System.platform() == Platform.Native);
```

#### `System.os_family(): OsFamily`

`Unix`, `Windows`, `Unknown`.

```dream
System.println(System.os_family() == OsFamily.Unix);
```

#### `System.is_browser(): bool`

```dream
System.println(System.is_browser());
```

#### `System.args(): string[]`

```dream
let argv = System.args();
System.println(argv.length);
```

#### `System.exe_path(): Option<string>`

```dream
System.println(System.exe_path().unwrap_or(""));
```

#### `System.env(name: string): Option<string>`

#### `System.env_or(name: string, fallback: string): string`

```dream
System.println(System.env("PATH").is_some());
System.println(System.env_or("MISSING", "default"));
```

#### `System.set_env(name, value): Result<bool, ArgError>`

```dream
switch (System.set_env("DEMO", "1")) {
    Ok(_) => System.println(System.env_or("DEMO", "")),
    Err(e) => System.println(e.message()),
}
```

#### `System.cwd(): Result<string, IoError>`

#### `System.set_cwd(path): Result<bool, IoError>`

```dream
switch (System.cwd()) {
    Ok(path) => System.println(path),
    Err(e) => System.println(e.code()),
}
```

## `to_string` and `hash_code`

Universal instance methods on every value:

```dream
let s = (42).to_string();      // "42"
let f = (3.14f).to_string();   // "3.14"
let h = "hello".hash_code();   // stable int for Map/Set
```

Override with `@override public fun to_string()` on a class. `print`/`println` and `+` auto-convert. See [The object type](../language/objects.md).

## Math

Static methods on `Math`. Numeric args coerce to `double`.

#### `Math.abs(x): double`

Returns the absolute value of `x`. Use for distances, magnitudes, and unsigned deltas.

```dream
System.println(Math.abs(0.0d - 3.5d));  // 3.5
```

#### `Math.floor(x)` / `Math.ceil(x)` / `Math.round(x): double`

Rounds toward negative infinity, positive infinity, or nearest integer respectively. Pick `floor`/`ceil` for discrete grid snapping; `round` for nearest whole number.

```dream
System.println(Math.floor(3.7d));  // 3.0
System.println(Math.ceil(3.2d));   // 4.0
System.println(Math.round(3.5d));  // 4.0
```

#### `Math.sqrt(x): Option<double>`

Returns the square root of non-negative `x`, or `None` for negative inputs. Safer than a panicking root for user-supplied values.

```dream
let hyp = Math.sqrt(3.0 * 3.0 + 4.0 * 4.0).unwrap_or(0.0d);  // 5.0
System.println(Math.sqrt(0.0d - 1.0d).is_none());             // true
```

#### `Math.pow(base, exponent): double`

Raises `base` to `exponent`. Use for exponentials and non-integer powers where `*` repetition is impractical.

```dream
System.println(Math.pow(2.0, 3.0));  // 8.0
```

#### `Math.sin` / `cos` / `tan` (radians)

Standard trigonometric functions; arguments are in radians. Use with `Math.PI` constants for angle conversion.

```dream
System.println(Math.sin(0.0d));  // 0.0
System.println(Math.cos(0.0d));  // 1.0
```

#### `Math.asin` / `acos` / `atan` / `atan2(y, x): double`

Inverse trig and two-argument arctangent. Prefer `atan2` over `atan(y/x)` when recovering an angle from Cartesian coordinates — it handles quadrants correctly.

```dream
System.println(Math.asin(1.0d));           // ~π/2
System.println(Math.atan2(1.0d, 1.0d));    // ~π/4
```

## `.length`

Element count on arrays, strings, `List`, `Map`, and `Set`:

```dream
System.println([10, 20, 30].length);  // 3
System.println("hello".length);       // 5
```

## `Buffer`

#### `Buffer.alloc<T>(len): T[]`

Zeroed fixed-length array. Prefer [`List<T>`](collections.md) for growable storage.

```dream
let buf = Buffer.alloc<int>(100);
```

`Buffer.realloc` / `Buffer.free` exist as `@unsafe` low-level helpers — not for normal application code.

## Timing (`Time` / `Stopwatch`)

Also documented under [DateTime](datetime.md).

#### `await Time.delay(ms: int): void`

Pauses for real wall-clock milliseconds. Use for frame pacing tied to real time (see also `Gpu.frame` for vsync-aligned demos).

```dream
await Time.delay(16);
```

#### `await Time.sleep(ms: int): void`

Cooperative async sleep on Dream's scheduler. Prefer in async code when other tasks should keep running.

```dream
await Time.sleep(100);
```

#### `Time.nano_time(): long`

Returns monotonic nanoseconds for high-resolution elapsed timing without calendar semantics.

```dream
let t0 = Time.nano_time();
let t1 = Time.nano_time();
System.println(t1 - t0);
```

#### `Stopwatch`

Measures elapsed wall time between `start` and `stop`. Prefer over manual `nano_time` differencing when you need pause/resume semantics.

```dream
let sw = Stopwatch();
sw.start();
// ... work ...
sw.stop();
System.println(sw.elapsed_ms());
sw.restart();
System.println(sw.is_running);
sw.reset();
```

| Member | Role |
|--------|------|
| `start()` / `stop()` | begin / pause |
| `reset()` / `restart()` | clear / clear+start |
| `elapsed_nanos()` / `elapsed_ms()` | elapsed while running or since stop |
| `is_running` | whether the watch is currently timing |
