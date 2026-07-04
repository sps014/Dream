# Imports

A Dream program can be split across multiple `.dream` files. Use `import` at the top of a file to pull in the declarations (functions, classes, enums) from another file.

## Importing a file

```dream
import math_lib;
```

- The path is a dotted module path (identifiers separated by `.`) ending with a semicolon.
- Each `.` maps to a directory separator, and the `.dream` extension is added automatically: `import utils.math_lib;` resolves to `utils/math_lib.dream`.
- The path is relative to the file that contains the `import`.
- Imported declarations become directly usable — no namespace prefix.

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

Imports are resolved recursively (an imported file may import others), and each file is processed only once even if imported from several places.

## Visibility

Dream uses a single keyword, `public`, that controls two independent axes.

### File/module visibility

A top-level declaration (function, class, interface, enum, or global variable) is **file-private by default**: it may be used freely anywhere within its own `.dream` file, but it is invisible to any other file, even one that `import`s it. Mark it `public` to make it part of the file's exported surface (and, for functions, to expose it to the host environment).

```dream
// lib.dream
public fun public_add(a: int, b: int): int { return a + b; }

fun helper(): int { return 99; }   // file-private: only usable inside lib.dream
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

A class member (field, instance method, static method, or property accessor) is **class-private by default**: it is accessible only from that class's own methods, regardless of file. Mark it `public` to make it accessible from outside the class. `static` never implies visibility on either axis — a `static` member must still be `public` to be called from outside its class.

```dream
public class Counter {
    count: int;                              // class-private field

    public fun value(): int { return this.count; }   // public method

    static fun make(): Counter {             // class-private static method
        return Counter();
    }
}
```

### How they compose

To use a member from another file you need **both**: the type must be `public` (so its name is reachable across files) and the member must be `public` (so it is accessible outside the class). A `public` function may not expose a class that is not itself `public`.

```dream
public class Point {
    public x: int;
    public y: int;
}

public fun origin(): Point {
    return Point(0, 0);
}
```

## Importing from JavaScript

Pulling in functions from the JavaScript host (rather than another `.dream` file) uses `extern fun` and is covered in [JS Interop](interop.md).
