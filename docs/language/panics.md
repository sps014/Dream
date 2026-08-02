# Panics

A **panic** is a fatal, non-recoverable runtime error: the program prints a message and halts immediately. There is no `try`/`catch` for panics (Dream has no exception mechanism at all) — a panic is closer to a Rust `panic!`/`abort` than a C#/Java exception. If you can anticipate a failure and want to handle it, use [`Option<T>`/`Result<T, E>`](../stdlib/option-result.md) instead; reach for a panic only for "this should never happen" conditions.

## What triggers a panic

The compiler inserts automatic checks for the operations below. Each prints a message and halts the instant the bad condition is detected:

| Situation | Example |
| --- | --- |
| Array or string index out of range (including negative) | `arr[arr.size()]`, `"abc"[-1]` |
| Integer division or remainder by zero | `10 / 0`, `10 % 0` |
| Casting an `object` to the wrong concrete type | `let o: object = "hi"; (int)o;` |

You can also panic explicitly:

```dream
System.panic("unreachable: config was never validated");
```

`System.panic(message: string): void` prints `message` and halts, exactly like an automatic check. Because it returns `void`, it can only be used in statement position — not as part of a larger expression.

## What a panic looks like

A panic prints its message to standard output, then halts the program. The automatic checks' messages are located with the failing source file, line, and declaring function, Rust-style, e.g.:

```
panic: index out of bounds (at /path/to/program.dream:6, in main)
```

`System.panic(message)` prints exactly the `message` you pass — no automatic location is appended, so include whatever context is useful yourself.

!!! note "Precision notes"
    The line is the checked construct's own source line whenever the compiler can determine it (`?` otherwise, e.g. for synthesized code with no source position). A checked construct inside a callee small enough to be inlined into its caller reports the caller's call-site line rather than its own, since inlining erases that distinction — an acceptable, still-diagnosable loss of precision, not a wrong (cross-file) location.

## Why panics, not undefined behavior

Before this mechanism existed, an out-of-bounds index or a bad unbox cast would silently read whatever bytes happened to be at the computed address — a real, exploitable bug rather than a diagnosable failure. Every one of the automatic checks above replaces that silent corruption with a deterministic, message-and-halt failure. This makes bugs *loud* during development instead of turning into mysterious wrong answers (or hard crashes) later on.
