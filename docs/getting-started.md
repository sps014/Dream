# Getting Started

This page gets you from nothing to a running Dream program in a few minutes.
**No Rust install required** for normal use — install prebuilt binaries like rustup.

## Install

### macOS / Linux

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sps014.github.io/Dream/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://sps014.github.io/Dream/install.ps1 | iex
```

That installs `dream`, `dreamer`, and `dream-lsp` under `~/.dream/bin` and hooks your shell
PATH (same layout as the contributor `use-toolchain.sh` script).

Open a new terminal, then check:

```bash
dream --help
dreamer --help
```

Pin a version with `DREAM_VERSION=0.1.0` before running the installer.

### Packages

Public packages live in [`sps014/dream-registry`](https://github.com/sps014/dream-registry)
([browse](https://sps014.github.io/dream-registry/)). Try:

```bash
dreamer search semver
dreamer add semver
```

See [Package Manager](tooling/package-manager.md).

### Building from source (contributors)

Only needed if you are hacking on the compiler itself (requires [Rust](https://rustup.rs)):

```bash
git clone https://github.com/sps014/Dream
cd Dream
source ./use-toolchain.sh    # builds + links dream / dreamer / dream-lsp
```

## Your first program

```bash
dreamer init hello && cd hello
```

Or create `hello.dream` yourself:

```dream
import system;

fun main() {
    System.println("Hello, world!");
}
```

Run it:

```bash
dreamer run
# or, without a dream.toml project:
dream run hello.dream
```

```
Hello, world!
```

`import system;` loads the console / process package so `System.println` is available. Other stdlib surfaces use their own packages (`system.collections`, `system.net`, …) — see [Imports](language/imports.md#standard-library-packages). The editor can insert these for you via an auto-import quick fix.

Compile without running:

```bash
dream hello.dream
```

That writes `.wat`, `.wasm`, and `.abi.json` next to your source.

### Running in the browser or Node

```bash
dreamer init hello --runtime web,node && cd hello
dreamer run --target web
dreamer run --target node
```

Or with the compiler directly:

```bash
dream --runtime --web hello.dream    # hello.web.runtime.js for the browser
dream --runtime --node hello.dream   # hello.node.runtime.js for Node ≥ 18
```

```javascript
import { run } from "./hello.web.runtime.js";  // or "./runtime/dream.js"
await run("hello.wasm");
```

See [JS Interop](language/interop.md#running-it-from-javascript) for marshaling, ABI details, and regenerating the full `runtime/dream.js` from `runtime/src/`.

## A bigger example

```dream
import system;

fun factorial(n: int): int {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

fun main() {
    let i = 1;
    while (i <= 10) {
        System.println(factorial(i));
        i = i + 1;
    }
}
```

A few things to notice:

- `fun` declares a function; its return type follows the `:`.
- The return type is optional when a function returns nothing, as in `fun main()`.
- `let` declares a local; its type is inferred from the initializer.
- `System.println` works on any type — `int`, `float`, `string`, `bool`, `char`, and your own classes.
- Conditions are parenthesized: `if (n <= 1)`.

## Where to go next

- [Variables](language/variables.md) — declaration, inference, and scope.
- [Control Flow](language/control-flow.md) — `if`, `while`, `for`, and `switch`.
- [Types & Data](language/types.md) — the full type landscape.
- [Classes & Structs](language/classes-structs.md) — define your own types with methods.
- [Collections](stdlib/collections.md) — `List<T>`, `Map<K, V>`, and `Set<T>` (`import system.collections;`).
- [GPU](stdlib/gpu.md) — WebGPU via `import system.gpu;` (see also [Compute shaders](language/compute.md)).
- [Imports](language/imports.md) — file imports and `system.*` packages.
- [Package Manager](tooling/package-manager.md) — `dreamer`, registries, and `semver`.
