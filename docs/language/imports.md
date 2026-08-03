# Imports & Modules

A Dream program can span several `.dream` files. `import` at the top of a file pulls in the public declarations — functions, classes, enums — of another file.

## Importing a file

```dream
import math_lib;
```

- The path is a dotted module path ending in a semicolon.
- Each `.` maps to a directory separator, and `.dream` is added automatically: `import utils.math_lib;` resolves to `utils/math_lib.dream`, relative to the importing file.
- Imported declarations are usable directly — there is no namespace prefix.

```dream
// math_lib.dream
public fun add_numbers(a: int, b: int): int {
    return a + b;
}
```

```dream
// main.dream
import math_lib;

fun main() {
    println(add_numbers(10, 20));   // 30
}
```

Imports resolve recursively (an imported file may import others), and each file is processed only once even if imported from several places.

## Visibility

Dream uses one keyword, `public`, for two independent axes.

### File / module visibility

A top-level declaration (function, class, interface, enum, or global) is **file-private by default** — usable anywhere in its own file but invisible to any other file, even one that imports it. Mark it `public` to export it (and, for functions, to expose it to the host):

```dream
// lib.dream
public fun public_add(a: int, b: int): int { return a + b; }
fun helper(): int { return 99; }   // file-private
```

```dream
// main.dream
import lib;

fun main() {
    System.println(public_add(2, 3));  // ok: public
    System.println(helper());          // error: 'helper' is not 'public'
}
```

### Class member visibility

A class member (field, method, static method, or accessor) is **class-private by default** — reachable only from that class's own methods, regardless of file. Mark it `public` to expose it. `static` never implies visibility; a `static` member must still be `public` to be called from outside the class:

```dream
public class Counter {
    count: int;                                     // class-private field
    public fun value(): int { return this.count; }  // public method
    static fun make(): Counter { return Counter(); } // class-private static
}
```

### How they compose

To use a member from another file you need **both**: the type must be `public` (so its name is reachable), and the member must be `public` (so it is accessible outside the class). A `public` function may not expose a non-`public` class.

```dream
public class Point {
    public x: int;
    public y: int;
}

public fun origin(): Point {
    return Point(0, 0);
}
```

### Why no `protected`/`internal`

Dream deliberately has only two visibility levels — file-private/public (Axis 1) and class-private/public (Axis 2) — and no `protected` or `internal`-style middle ground. Both of those modifiers exist in other languages to solve problems Dream's design doesn't have:

- **`protected`** (accessible to subclasses) has no reason to exist because Dream [has no class inheritance](classes-structs.md) — classes cannot extend one another, only implement interfaces (which contribute no member implementations to override or share). With no derived-class hierarchy, there is nothing for "accessible to this class and its subclasses" to mean.
- **`internal`** (accessible package/assembly-wide but not beyond it) has no reason to exist because Dream has no package/assembly grouping above the individual file: a program is just a set of `.dream` files linked by `import`, with no intermediate module boundary for "internal" to scope to. File-private (the default) already gives the tightest useful grouping, and `public` already gives the widest; there is no third granularity in between for `internal` to occupy.

If a future version of Dream gains class inheritance or a package system, revisiting this is reasonable — but retrofitting either keyword today would add surface area with no corresponding semantics to back it.

## Importing from JavaScript

Pulling in functions from the JavaScript host (rather than another `.dream` file) uses `extern fun`. See [JS Interop](interop.md).
