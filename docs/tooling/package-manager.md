# Package Manager (`dreamer`)

`dreamer` is Dream's package manager: it reads/writes the `dream.toml` project manifest, resolves
dependencies into a `dream.lock` lockfile, materializes them into a project's `dream_packages/`
directory, and wraps the `dream` compiler for `build`/`run`. It lives at
[`tooling/dreamer`](https://github.com/sps014/Dream/tree/main/tooling/dreamer) as its own Rust
crate/binary, separate from the compiler itself.

## Installing `dreamer`

From a checkout of the Dream repo:

```bash
cargo install --path tooling/dreamer
```

This installs a `dreamer` binary onto your `PATH`. `dreamer build`/`dreamer run` also need the
`dream` compiler binary discoverable — either also on `PATH`, via the `DREAM_BIN` environment
variable, or (while developing inside this repo) simply already built at `target/debug/dream` or
`target/release/dream`.

## The manifest: `dream.toml`

Every Dream project managed by `dreamer` has a `dream.toml` at its root, analogous to `Cargo.toml`
or `pyproject.toml`:

```toml
[package]
name = "myapp"
version = "0.1.0"
edition = "2026"
authors = ["Jane Doe <jane@example.com>"]
description = "My Dream app"
entry = "src/main.dream"        # compiler entry point, relative to this file
license = "MIT"
targets = ["native", "web"]     # optional hosts: native, web, node (omit = no preference)

[dependencies]
http-utils = "1.2"                                    # semver requirement, resolved from a registry
json-tools = { version = "0.3", registry = "default" }
local-lib  = { path = "../local-lib" }                 # local path dependency
vendored   = { git = "https://github.com/user/vendored-dream", tag = "v1.0.0" }

[dev-dependencies]
test-utils = "0.4"

[scripts]
start = "dreamer run"

[registries]
default = "https://registry.dream-lang.org"    # overridable; also supports file:// for private/offline registries
```

- `[package].entry` is the file `dreamer build`/`dreamer run` hand to the `dream` compiler.
- `[package].targets` is an optional list of hosts this project supports: `native` (wasmtime via
  `dream run`), `web` (browser + `*.web.runtime.js`), and/or `node` (Node ≥ 18 + `*.node.runtime.js`).
  Omit the field (or leave it empty) for today's free-choice behavior — `dreamer run` defaults to
  native. Combinations are allowed; see `dreamer run` below for how the host is chosen.
- A dependency is either a bare semver requirement string, or a table with exactly one of
  `path`, `git`, or `version` (+ optional `registry`).
- Package names follow the same shape as Cargo crate names (letters, digits, `-`, `_`, starting
  with a letter). A hyphen in the name maps to an underscore in `import` statements — `json-tools`
  is imported as `import json_tools...;` — mirroring how a hyphenated Rust crate name is referenced
  via `use` with underscores.
- `[registries]` maps registry aliases to base URLs; a dependency's `registry = "..."` picks one,
  defaulting to the `default` alias.
- `[scripts]` is currently informational project metadata — no `dreamer` subcommand executes it
  yet, but it's a stable place to document how a project is normally built/run/tested.

## The lockfile: `dream.lock`

`dreamer install` (and every command that implies it) writes `dream.lock`: the exact, pinned
dependency graph, analogous to `Cargo.lock`/`package-lock.json`. It should be committed to version
control for applications so every checkout resolves to byte-identical dependency versions.

```toml
version = 1

[[package]]
name = "json-tools"
version = "0.3.1"
source = "registry+https://registry.dream-lang.org"
checksum = "sha256:9f2c...ab31"
dependencies = []
```

`source` is one of `registry+<url>`, `git+<url>#<rev>`, or `path+<path>`. Packages are always
written sorted by name so the file diffs cleanly and resolution order never affects its contents.

Re-running `dreamer install` prefers versions already pinned in an existing `dream.lock` (as long
as they still satisfy every requirement in `dream.toml`), so it never silently upgrades a
dependency just because a newer version was published — use `dreamer update` for that.

## `dream_packages/`

Every dependency is materialized under `dream_packages/<name-with-underscores>/` next to
`dream.toml` — a local path dependency is symlinked (so edits show up immediately), while registry
and git dependencies are copied from a shared, checksum-verified download cache at
`~/.dream/registry/`. `dream_packages/` is never committed (`dreamer init` adds it to
`.gitignore`); it's fully reproducible from `dream.toml` + `dream.lock`.

When a plain `import` doesn't resolve to a local file, the compiler looks under `dream_packages/`:

- `import json_tools;` (no dot) looks for `dream_packages/json_tools/src/json_tools.dream` — a
  package's self-named entry file.
- `import json_tools.parse;` looks for `dream_packages/json_tools/src/parse.dream`.

See [Imports & Modules](../language/imports.md) for the base (non-package) import syntax.

The LSP suggests installed package names when you type `import `, reading from `dream_packages/` —
no separate configuration needed.

## Registry protocol

A registry is a sparse index plus tarball storage. A plain directory works, served as a local
`file://` path or over any static HTTP file server:

```text
<base>/index/<name>                         newline-delimited JSON, one entry per published version
<base>/dl/<name>/<name>-<version>.tar.gz    the tarball an index entry's "tarball" field points at
```

Each line of `<base>/index/<name>` is a JSON object:

```json
{"name":"json-tools","vers":"0.3.1","deps":[{"name":"buffer-utils","req":"^1.0"}],"cksum":"sha256:...","tarball":"dl/json-tools/json-tools-0.3.1.tar.gz","description":"JSON helpers"}
```

Optional endpoints for `dreamer search` / `dreamer publish` against HTTP registries:

- `GET  <base>/search?q=<query>` → JSON array of index-entry objects
- `POST <base>/api/v1/publish` → JSON body `{ "entry": <index-entry>, "tarball_base64": "..." }`

No production Dream registry is hosted yet. Point `[registries] default` at any location
implementing the protocol above, including a `file://` directory for local or private use.

## Dependency resolution

Registry dependencies resolve to the highest version that satisfies every accumulated requirement.
`path` and `git` dependencies are pinned from their own `dream.toml` and are never subject to
registry version selection. Conflicting requirements produce a clear error naming both sides.

## Command reference

| Command | Effect |
|---|---|
| `dreamer init [name] [--runtime native,web,node] [--dir <path>]` | Scaffold `dream.toml` + `src/main.dream`, a richer `.gitignore`, and (when `--runtime` includes them) `index.html` / `run.mjs` linked to selective `*.web.runtime.js` / `*.node.runtime.js`. |
| `dreamer add <name> [--version <req>] [--path <dir>] [--git <url> [--tag/--branch/--rev <ref>]] [--dev]` | Add (or update) a dependency in `dream.toml`, then resolve and install. |
| `dreamer remove <name>` | Remove a dependency from `dream.toml` and `dream_packages/`, then re-resolve. |
| `dreamer install` | Resolve `dream.toml` (respecting `dream.lock` where still compatible) and materialize `dream_packages/`. |
| `dreamer update [<name>]` | Re-resolve to the latest compatible version(s); with a name, only that package is allowed to move. |
| `dreamer build [--release]` | Install, then compile `[package].entry`. When `targets` includes `web` and/or `node`, forwards `--runtime --web` / `--node` so selective runtime siblings are emitted. |
| `dreamer run [--target native\|web\|node] [-- <args>]` | Install, then run on the resolved host (see below). |
| `dreamer publish [--registry <url>]` | Package the current project and publish it to a registry. |
| `dreamer search <query>` | Search a registry for packages by name. |
| `dreamer tree` | Print the resolved dependency tree from `dream.lock`. |

### How `dreamer run` picks a host

| `package.targets` | No `--target` | With `--target X` |
|---|---|---|
| empty / omitted | **native** (`dream run`) | any of `native` / `web` / `node` (ad-hoc escape hatch) |
| exactly one entry | that host | must match that host |
| two or more | error — require `--target` | `X` must be one of the listed targets |

Per host:

- **native** — `dream run <entry> [args…]` (wasmtime).
- **node** — compile with `--runtime --node`, then `node run.mjs` from the project root.
- **web** — compile with `--runtime --web`, then serve the project root and print
  `http://127.0.0.1:<port>/index.html` (Ctrl-C to stop).

## Walkthrough

```bash
# scaffold a new project (optional hosts)
dreamer init myapp --runtime web,node && cd myapp

# add a dependency from the default registry
dreamer add json-tools --version "^0.3"

# add a local sibling project during development
dreamer add local-lib --path ../local-lib

# install everything into dream_packages/, generate dream.lock
dreamer install
```

```dream
// src/main.dream
import json_tools;
import local_lib;
import system;

fun main(): void {
    System.println(hello());   // from json_tools/src/json_tools.dream
}
```

```bash
# compile and run (multi-target projects need --target)
dreamer run --target node
dreamer run --target web

# see what's actually installed
dreamer tree

# bump json-tools to the newest version satisfying dream.toml
dreamer update json-tools

# publish this project itself to a registry
dreamer publish --registry https://registry.dream-lang.org
```

## Trying it end to end without a hosted registry

Since a plain directory is a fully compliant registry, you can try the whole flow locally:

```bash
mkdir -p /tmp/my-registry
# ... publish a package into it, e.g. by running `dreamer publish` from that package's own
# project with `--registry file:///tmp/my-registry` ...

# then, in a consuming project's dream.toml:
# [registries]
# default = "file:///tmp/my-registry"
dreamer add that-package
```

This is exactly the fixture setup exercised by `tooling/dreamer`'s own integration tests.

## For contributors

Import resolution, the registry protocol wire format, and resolver edge cases live in the
[Contributing](../compiler/README.md) handbook and the `tooling/dreamer` crate — not required for
day-to-day package use.
