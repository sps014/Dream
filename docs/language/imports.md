# Imports & Modules

A Dream program can span several `.dream` files. `import` at the top of a file pulls in the public declarations — functions, classes, enums — of another file (or of an embedded stdlib package).

## Standard library packages

Opt-in stdlib APIs live under the reserved `system.*` package tree. A plain import loads that package (and its dependencies) into the program; public names are then usable unqualified:

```dream
import system;                 // System, DateTime, Stopwatch, Random, …
import system.collections;     // List, Map, Set, Queue, Stack
import system.text;            // string methods, StringBuilder, Regex
import system.json;            // Json, JsonValue
import system.net;             // HttpClient, HttpResponse, HttpHeaders, Url, …
import system.io;              // File, FileStream, Path, IoError
import system.encoding;        // Encoding (UTF-8 / hex / Base64)
import system.logging;         // Logger, LogLevel, handlers
import system.crypto;          // Sha256, HmacSha256, SecureRandom
```

Always available without an import (bootstrap): `Option`, `Result`, `Error`, `ParseError`, `Buffer`, `Bytes`, `Span`, `Pointer`, `Promise`, `WebWorker`, `Math`, `js`, comparison/`Collection` interfaces, and primitive `extend` methods (`int.parse`, `bool.parse`, …). Low-level `string.alloc` / `string.set` are also bootstrap; higher-level string helpers require `import system.text;` (or `import system;`, which depends on text).

There is no `import system.*;` wildcard — import each package you need (the editor offers an auto-import quick fix when you type an unresolved stdlib name).

Reserved `system` / `system.*` paths always resolve to the embedded stdlib, not a local `system/…` file.

## Importing a file

```dream
import math_lib;
```

- The path is a dotted module path ending in a semicolon.
- Each `.` maps to a directory separator, and `.dream` is added automatically: `import utils.math_lib;` resolves to `utils/math_lib.dream`, relative to the importing file.
- Imported declarations are usable directly — there is no namespace prefix.
- If no matching file exists relative to the importing file, resolution falls back to a
  `dream_packages/` dependency directory installed by the [`dreamer` package manager](../tooling/package-manager.md) — so `import json_tools;` can resolve to a project dependency
  once `dreamer install` has run, with no different syntax required.
- Names that match a stdlib package (`system`, `system.net`, …) never fall through to the filesystem.

```dream
// math_lib.dream
public fun add_numbers(a: int, b: int): int {
    return a + b;
}
```

```dream
// main.dream
import math_lib;
import system;

fun main() {
    System.println(add_numbers(10, 20));   // 30
}
```

Imports resolve recursively (an imported file may import others), and each file is processed only once even if imported from several places.

## Modules

A file can opt into a named **module** by declaring it at the very top, before any `import`:

```dream
// utils/math_lib.dream
module utils.math;

public fun add(a: int, b: int): int {
    return a + b;
}
```

- At most one `module` declaration per file, and it must be the first thing in the file (before `import` and before any other declaration).
- A module path is purely a name — `module utils.math;` does **not** need to live in a `utils/math` directory. File resolution for plain `import`s is still directory-based, exactly as before; `module` only tags a file's declarations with a logical namespace, independent of where the file sits on disk.
- Files that don't declare a `module` stay in the implicit, unnamed **root module** — today's flat, unqualified behavior is unchanged for any file that never writes `module`.
- Two files in **different** modules may declare the same name (e.g. two `public fun add(...)`) without colliding. Two declarations in the **same** module (or both in the unnamed root module) with the same name still collide exactly as before — a module is still one namespace.
- Same-module overloads and cross-module duplicate names compose: within one module you may overload `add` by signature; across modules each side keeps its own overload set. Call sites and `import … as` aliases still use the source names.

## Aliased imports (resolving duplicate names)

Because two modules can legally define the same name, `import` has a second form — with a trailing `as` clause — that pulls in one specific item from a *module* (not a file) under a chosen local alias:

```dream
import <dotted-module-path>.<item> as <alias>;
```

```dream
// vendor/math_lib.dream
module vendor.math;

public fun add(a: float, b: float): float {
    return a + b;
}
```

```dream
// main.dream
import system;
import utils.math_lib;               // file import, unchanged: loads utils/math_lib.dream
import vendor.math_lib;              // loads vendor/math_lib.dream

import utils.math.add as add_int;    // aliases the `add` declared in module `utils.math`
import vendor.math.add as add_float; // aliases the `add` declared in module `vendor.math`

fun main() {
    System.println(add_int(1, 2));           // 3
    System.println(add_float(1.5, 2.5));     // 4.0
}
```

The two `import` forms are told apart by the trailing `as` clause, not by a different keyword:

- `import <dotted-file-path>;` (no `as`) — resolves against the filesystem, exactly as in [Importing a file](#importing-a-file). The declaring file must still be pulled into the program this way (directly or transitively) before an aliased import from its module can see it.
- `import <module-path>.<item> as <alias>;` (with `as`) — resolves `<item>` against the *declared module* `<module-path>`, not the filesystem, and binds it into the current file's top-level scope as `<alias>`.

The aliased item must be `public` or `internal`; importing a `private` declaration this way is always an error, regardless of which module the importing file belongs to. If `<alias>` collides with another name already in scope in the importing file, that's a normal "already defined" diagnostic — pick a different alias.

There is no wildcard `import pkg.*;` in this version — only single-item aliased imports. This affects convenience, not reachability: it just means one `import ... as` line per name you want to consume unqualified from another module (mirroring Rust/Go's general avoidance of star-imports).

## Visibility

Dream has three visibility levels — `private` (the default, no keyword), `internal`, and `public` — applied consistently across two independent axes.

### File / module visibility

A top-level declaration (function, class, interface, enum, or global) is **private by default** — usable anywhere in its own file but invisible to any other file, even one that imports it.

- `internal` — visible from any file that shares the same declaring `module` (or, for undeclared files, the shared unnamed root module), but not from a file in a different module.
- `public` — visible everywhere the file is reachable from (and, for functions, exposed to the host).

```dream
// lib.dream
module utils.math;

public fun public_add(a: int, b: int): int { return a + b; }
internal fun shared_helper(): int { return 99; }  // visible anywhere in module utils.math
fun private_helper(): int { return 1; }           // file-private
```

```dream
// other_file_same_module.dream
module utils.math;   // same module as lib.dream

fun uses_helper(): int {
    return shared_helper();  // ok: same module
}
```

```dream
// main.dream (unnamed root module)
import lib;

fun main() {
    System.println(public_add(2, 3));   // ok: public
    System.println(shared_helper());    // error: 'shared_helper' is internal to module utils.math
    System.println(private_helper());   // error: 'private_helper' is not 'public'
}
```

### Class member visibility

A class member (field, method, static method, accessor, or **constructor**) is **class-private by
default** — reachable only from that class's own methods, regardless of file. `static` never implies
visibility; a `static` member must still be `internal`/`public` to be called from outside the class.
Constructors follow the same rule: mark them `public` (or `internal`) to allow `Type(...)` from
outside the type. An implicit zero-arg default (no `constructor` declared) is public. Destructors
(`del`) are always private.

- `internal` — reachable from anywhere in the declaring class's module (not just the class's own methods), but not outside the module.
- `public` — reachable from anywhere the type itself is reachable.

```dream
module utils.math;

public class Vector {
    internal x: int;                          // visible anywhere in module utils.math
    public fun length_sq(): int {
        return helper_square(this.x);
    }
}
```

### How they compose

To use a member from another file you need **both**: the type must be reachable from the caller (its axis-1 visibility, evaluated against the caller's module), and the member must be reachable too (its axis-2 visibility, evaluated the same way). Effective accessibility from a given caller is the **minimum** of the two — an `internal class` only exposes its `public` members within the declaring module; even a `public` member is invisible outside it, because the type itself isn't reachable there.

```dream
public class Point {
    public x: int;
    public y: int;
}

public fun origin(): Point {
    return Point(0, 0);
}
```

There is no `protected` modifier. Dream has [no class inheritance](classes-structs.md) — only interface implementation — so there is no subclass hierarchy for `protected` to grant access to.

## Importing from JavaScript

Pulling in functions from the JavaScript host (rather than another `.dream` file) uses `extern fun`. See [JS Interop](interop.md).
