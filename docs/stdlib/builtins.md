# Built-ins

These functions and types are available in every Dream program with no import. They cover console I/O, the universal `to_string`/`hash_code` methods, `Math`, and low-level buffers.

## Console output

`System.print(value)` writes any value to stdout with no trailing newline; `System.println(value)` adds one. Both are generic over the value type — you never convert first, and classes with an overridden `to_string` are handled automatically.

```dream
System.print(42);         // "42"
System.println("hello");  // "hello\n"
System.println(true);     // "true\n"
```

`System.printColored(text, color)` prints one string in a color and resets after (no newline). `System.setForeground`/`setBackground(color)` change the color of all subsequent output until `System.resetColor()`. `System.clear()` clears the terminal.

`ConsoleColor` is an enum of the 16 standard console colors (C# ordering): `Black`, `DarkBlue`, `DarkGreen`, `DarkCyan`, `DarkRed`, `DarkMagenta`, `DarkYellow`, `Gray`, `DarkGray`, `Blue`, `Green`, `Cyan`, `Red`, `Magenta`, `Yellow`, `White`.

```dream
System.printColored("warning", ConsoleColor.Yellow);
```

These use ANSI escapes, supported by every macOS/Linux terminal and Windows 10+ console (native builds enable Windows virtual-terminal processing at startup).

## Console input

- `System.readLine()` — blocks until a full line is available on stdin, returns it without the trailing newline.
- `System.readKey()` — blocks for a single keypress and returns its character code, without waiting for Enter or echoing. Keys with no character (e.g. arrows) yield `(char)0`. In the browser and for non-interactive stdin (piped input), it falls back to reading a single byte.
- `System.readInt()` / `System.readDouble()` — read a line and parse it, returning a `Result` so a malformed line is `Err` rather than a crash.

```dream
System.print("age? ");
switch (System.readInt()) {
    Ok(v)  => System.println("age: " + v.to_string()),
    Err(e) => System.println("invalid input: " + e),
}
```

`System.exit(code)` terminates the process immediately and never returns.

## `to_string` and `hash_code`

Both are universal instance methods on every value:

```dream
let s = (42).to_string();      // "42"
let f = (3.14f).to_string();   // "3.14"
let h = "hello".hash_code();   // a stable int, used internally by Map/Set
```

A class with `@override public fun to_string()` uses that method. You rarely call `to_string` explicitly: `print`/`println` convert any value, and `+` auto-converts the non-string operand, so `"x = " + 42` yields `"x = 42"`. See [The object type](../language/objects.md) for overriding these.

## Math

Math functions are static methods on `Math`. Each accepts numeric arguments (coerced to `double`) and returns `double`.

| Function | Description |
|----------|-------------|
| `Math.sin` / `cos` / `tan` | Trigonometry (radians) |
| `Math.sqrt` | Square root |
| `Math.abs` | Absolute value |
| `Math.pow` | Power (x^y) |
| `Math.floor` / `ceil` / `round` | Rounding |

`Math.sqrt` returns an `Option<double>` — `None` for a negative argument, otherwise `Some(root)`. The rest return a plain `double`:

```dream
let hyp = Math.sqrt(3.0 * 3.0 + 4.0 * 4.0).unwrap_or(0.0d);  // 5.0
let bad = Math.sqrt(-1.0d).is_none();                        // true
let p = Math.pow(2.0, 3.0);                                  // 8.0
```

## `size`

`size()` is the element-count method shared by arrays, strings, `List`, `Map`, and `Set`, so every collection is queried the same way:

```dream
System.println([10, 20, 30].size());   // 3
System.println("hello".size());        // 5
```

## `Buffer.alloc`

Allocates a zeroed, fixed-length `T[]` of a given size — the low-level primitive the collections build on. Reach for it only when you need a raw array whose size isn't known at compile time; otherwise prefer the growable [`Array<T>`](../language/arrays.md).

```dream
let buf = Buffer.alloc<int>(100);   // int[] with 100 zero-initialized slots
```
