# Built-in Functions

These are available in every Dream program without any import.

## System.print

Prints a value to stdout without a trailing newline. Works on all types.

```dream
System.print(42);         // prints "42"
System.print(3.14f);      // prints "3.14"
System.print("hello");    // prints "hello"
System.print(true);       // prints "true"
System.print('A');        // prints "A"
```

For classes that override `to_string`, `System.print` calls the override automatically. Because
`print`/`println` are generic over the value type, you never need to convert first — pass the value
directly.

## System.println

Like `System.print`, but appends a newline (`\n`) after the value.

```dream
System.println(42);       // prints "42\n"
System.println("hello");  // prints "hello\n"
```

## System.readLine

Blocks until a full line of text is available on stdin, and returns it without the trailing
newline.

```dream
System.print("name? ");
let name = System.readLine();
System.println("hi " + name);
```

## System.readKey

Blocks until a single keypress is available and returns its character code, without waiting for
Enter and without echoing it back to the terminal. Keys with no character representation (e.g.
arrow keys) yield `(char)0`. In the browser JS host and when stdin is not an interactive terminal
(e.g. piped input), this falls back to reading a single byte instead of a true raw keypress.

```dream
System.print("press a key: ");
let k = System.readKey();
System.println("you pressed: " + k.to_string());
```

## System.readInt / System.readDouble

Read a line from stdin and parse it as an `int`/`double`, returning a `Result` so a malformed line
is `Err` instead of crashing.

```dream
System.print("age? ");
switch (System.readInt()) {
    Ok(v)  => System.println("age: " + v.to_string()),
    Err(e) => System.println("invalid input: " + e),
}
```

## System.exit

Terminates the process immediately with the given exit code. Never returns.

```dream
System.exit(1);
```

## System.clear

Clears the terminal screen and moves the cursor to the top-left, via ANSI escapes.

```dream
System.clear();
```

## ConsoleColor and colored output

`ConsoleColor` is a plain enum with the 16 standard console colors (matching C#'s `ConsoleColor`
ordering): `Black`, `DarkBlue`, `DarkGreen`, `DarkCyan`, `DarkRed`, `DarkMagenta`, `DarkYellow`,
`Gray`, `DarkGray`, `Blue`, `Green`, `Cyan`, `Red`, `Magenta`, `Yellow`, `White`.

`System.setForeground`/`System.setBackground` emit an ANSI escape that changes the color of all
subsequent output until `System.resetColor()` is called. `System.printColored` prints one string in
a color and resets immediately after (no trailing newline):

```dream
System.setForeground(ConsoleColor.Green);
System.println("success");
System.resetColor();

System.printColored("warning", ConsoleColor.Yellow);
```

These rely on ANSI escape sequences, which every macOS/Linux terminal and Windows 10+ console
support; on native builds, Windows virtual-terminal processing is enabled automatically at
startup.

## to_string

`to_string()` is a universal instance method available on every value, returning its string
representation:

```dream
let s = (42).to_string();      // "42"
let b = (true).to_string();    // "true"
let f = (3.14f).to_string();   // "3.14"
```

For classes with a `@override public fun to_string()` method, that method is called.

You rarely need to call it explicitly: `System.print`/`println` already convert any value, and
string concatenation auto-converts the non-string operand, so `"x = " + 42` yields `"x = 42"`.

## hash_code

`hash_code()` is a universal instance method returning a stable `int` hash for any value:

```dream
let h = "hello".hash_code();
let h2 = (42).hash_code();
```

Used internally by `Map<K, V>` to find buckets.

## Math

Math functions are static methods on the `Math` class. Each function accepts numeric arguments (coerced to `double`) and always returns `double`.

| Function     | Description      |
|--------------|------------------|
| `Math.sin`   | Sine (radians)   |
| `Math.cos`   | Cosine (radians) |
| `Math.tan`   | Tangent (radians)|
| `Math.sqrt`  | Square root      |
| `Math.abs`   | Absolute value   |
| `Math.pow`   | Power (x^y)      |
| `Math.floor` | Floor            |
| `Math.ceil`  | Ceiling          |
| `Math.round` | Round to nearest |

`Math.sqrt` returns an `Option<double>` — `None` for a negative argument (which has no real square root), otherwise `Some(root)`. The other functions above return a plain `double`.

```dream
let hyp = Math.sqrt(3.0 * 3.0 + 4.0 * 4.0).unwrap_or(0.0d);  // 5.0
let bad = Math.sqrt(-1.0d).is_none();                        // true
let s = Math.sin(0);                                // 0.0 (int argument coerced to double)
let p = Math.pow(2.0, 3.0);                         // 8.0
```

## size

`size` is the element-count method on arrays and strings. It is the same `size()` the stdlib
`List` and `Map` expose, so every collection is queried the same way:

```dream
let arr = [10, 20, 30];
System.println(arr.size());     // 3

let name = "hello";
System.println(name.size());    // 5
```

## Buffer.alloc

Allocates a zeroed, fixed-length `T[]` buffer of a given size. This is the low-level primitive the
standard-library collections build on. Reach for it directly only when you need a raw array whose
size isn't known at compile time; otherwise prefer the growable [`Array<T>`](../language/arrays.md)
collection.

```dream
let buf = Buffer.alloc<int>(100);   // int[] with 100 zero-initialized slots
```
