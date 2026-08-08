# Package Manager (`dreamer`)

`dreamer` is Dream's package manager: it reads/writes the `dream.toml` project manifest, resolves
dependencies into a `dream.lock` lockfile, materializes them into a project's `dream_packages/`
directory, and wraps the `dream` compiler for `build`/`run`. It lives at
[`tooling/dreamer`](https://github.com/sps014/dream/tree/main/tooling/dreamer) as its own Rust
crate/binary, separate from the compiler itself.

## Installing `dreamer`

From a checkout of the Dream repo:

```bash
cargo install --path tooling/dreamer
```

This installs a `dreamer` binary onto your `PATH`. `dreamer build`/`dreamer run` also need the
`dream` compiler binary discoverable. Resolution order:

1. `DREAM_BIN` — exact path to the `dream` binary
2. `DREAM_HOME` — directory containing `dream` (typical dev value: `<repo>/target/debug`)
3. `dream` on `PATH`
4. Sibling of the `dreamer` executable / walk for `target/{debug,release}/dream`

For tooling that needs to invoke the package manager itself, set `DREAMER_HOME` to the directory
containing `dreamer` (often the same `target/debug` or `target/release` folder). From the Dream
repo root you can do both the build and the exports in one step:

```bash
source ./use-toolchain.sh          # release (default)
# source ./use-toolchain.sh --debug
```

The VS Code extension mirrors these via settings `dream.home` and `dreamer.home`. The `.vsix`
does not ship compiler binaries — launch the editor from a shell that has sourced
`./use-toolchain.sh`, or set those settings explicitly.

## The manifest: `dream.toml`

Every Dream project managed by `dreamer` has a `dream.toml` at its root, analogous to `Cargo.toml`
or `pyproject.toml`:

```toml
[package]
name = "myapp"
version = "0.1.0"
type = "bin"                    # or "lib" (default: bin)
edition = "2026"
authors = ["Jane Doe <jane@example.com>"]
description = "My Dream app"
entry = "src/main.dream"        # required for bin; forbidden for lib
license = "MIT"
keywords = ["http", "json"]         # optional; used by dreamer search / catalog.json
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
default = "https://raw.githubusercontent.com/sps014/dream-registry/main"
```

- `[package].type` is `bin` (default) or `lib`. Libraries omit `entry`, are not runnable (`dreamer run` /
  `dreamer pack` error), and are typechecked via the conventional `src/<import_segment>.dream` root
  (`http-utils` → `src/http_utils.dream`). Binaries require `entry` and a top-level `main`.
- `[package].entry` is the file `dreamer build`/`dreamer run` hand to the `dream` compiler (**bin only**).
- Package builds emit under `target/debug/` (default) or `target/release/` (`--release`), not beside
  sources. Bare `dream file.dream` compiles without a `dream.toml` still emit siblings next to the
  source file.
- **Web/Node hosts use stable aliases** `target/web/` and `target/node/` (refreshed by
  `dreamer build` / `dreamer run` from the active profile). Scaffolded `index.html` / `run.mjs`
  always import those paths — no need to edit them when switching debug ↔ release. Existing
  projects that hardcode `target/debug/…` should retarget once to `target/web/` / `target/node/`.
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
source = "registry+https://raw.githubusercontent.com/sps014/dream-registry/main"
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
{
  "name": "json-tools",
  "vers": "0.3.1",
  "deps": [{"name": "buffer-utils", "req": "^1.0"}],
  "cksum": "sha256:...",
  "tarball": "dl/json-tools/json-tools-0.3.1.tar.gz",
  "description": "JSON helpers",
  "authors": ["Jane Doe <jane@example.com>"],
  "license": "MIT",
  "edition": "2026",
  "type": "lib",
  "targets": ["native", "web"],
  "readme": "README.md",
  "keywords": ["json", "parse"]
}
```

Fields beyond `name` / `vers` / `deps` / `cksum` / `tarball` are optional metadata copied from
`dream.toml` at publish time. `readme` is an **archive-relative path** (e.g. `README.md`) pointing at
the README packed into the tarball — the index never embeds README body text.
`keywords` come from `[package].keywords` and feed static search.
`catalog.json` stores the same discovery fields except `deps` / `cksum` / `tarball`.

Optional endpoints / files for `dreamer search` / `dreamer publish`:

- `GET  <base>/search?q=<query>` → JSON array of index-entry objects (dynamic registries)
- `GET  <base>/catalog.json` → compact search catalog used when `/search` is absent (static/GitHub registries)
- `POST <base>/api/v1/publish` → JSON body `{ "entry": <index-entry>, "tarball_base64": "..." }` (non-GitHub HTTP registries)

The default public registry is the GitHub repo
[`sps014/dream-registry`](https://github.com/sps014/dream-registry), served at
`https://raw.githubusercontent.com/sps014/dream-registry/main`. Indexes live under `index/`,
tarballs under `dl/` (separate trees). Max package tarball size is **10 MiB**.

`dreamer publish` to that registry uses the GitHub Contents API (set `DREAM_REGISTRY_TOKEN` or
`--token` with `contents:write`). Point `[registries] default` at any other `file://` or
`http(s)://` location implementing the protocol above for private/offline use.

### Static search (cargo-like)

There is no live search API. Discovery is fully static:

- **CLI:** `dreamer search <query>` downloads `catalog.json` and matches against package **name**,
  **description**, and **keywords** (case-insensitive substring) — the Dream equivalent of
  `cargo search`.
- **Browser:** [sps014.github.io/dream-registry](https://sps014.github.io/dream-registry/) filters
  the same `catalog.json` client-side.

The first published library is [`semver`](https://github.com/sps014/dream/tree/main/packages/semver)
(`dreamer add semver`).

## Dependency resolution

Registry dependencies resolve to the highest version that satisfies every accumulated requirement.
`path` and `git` dependencies are pinned from their own `dream.toml` and are never subject to
registry version selection. Conflicting requirements produce a clear error naming both sides.

## Command reference

| Command | Effect |
|---|---|
| `dreamer init [name] [--lib] [--runtime native,web,node] [--dir <path>]` | Scaffold `dream.toml` + source stub (`src/main.dream` for bins, `src/<name>.dream` for `--lib`), `.gitignore` (`dream_packages/`, `target/`), and (when `--runtime` includes them) `index.html` / `run.mjs` linked to stable `target/web/` / `target/node/` aliases. |
| `dreamer add <name> [--version <req>] [--path <dir>] [--git <url> [--tag/--branch/--rev <ref>]] [--dev]` | Add (or update) a dependency in `dream.toml`, then resolve and install. |
| `dreamer remove <name>` | Remove a dependency from `dream.toml` and `dream_packages/`, then re-resolve. |
| `dreamer install` | Resolve `dream.toml` (respecting `dream.lock` where still compatible) and materialize `dream_packages/`. |
| `dreamer update [<name>]` | Re-resolve to the latest compatible version(s); with a name, only that package is allowed to move. |
| `dreamer build [--release]` | Install, then compile the package root (`entry` for bins; conventional lib root for libs) with `--crate-type`. Artifacts land in `target/debug` or `target/release`. When `targets` includes `web` and/or `node`, also refreshes `target/web/` / `target/node/` aliases from that profile. |
| `dreamer run [--release] [--port <n>] [--target native\|web\|node] [-- <args>]` | Install, then run on the resolved host (see below). `--release` uses the release profile (and refreshes web/node aliases). Web serves on port **8787** by default (override with `--port`); a second run restarts the previous server on that port. Errors on `type = "lib"`. |
| `dreamer pack [--target <os>-<arch>\|all]…` | Release-build a **bin** package and embed its `.wasm` in a native `dream-runner` host → `target/pack/<name>-<os>-<arch>[.exe]`. Default target is the host OS/arch. Distinct from registry `publish`. |
| `dreamer publish [--registry <url>] [--token <tok>]` | Package source (`dream.toml` + `src/`) and publish it to a registry (≤10 MiB). |
| `dreamer search <query>` | Search the registry catalog by name / description / keywords (static `catalog.json`). |
| `dreamer tree` | Print the resolved dependency tree from `dream.lock`. |

### Native `dreamer pack`

Produces a single native executable per selected platform by embedding the release `.wasm` in the
workspace `dream-runner` crate (wasmtime + the same host ABI as `dream run`).

```bash
dreamer pack                         # host → target/pack/<name>-<os>-<arch>
dreamer pack --target linux-x64
dreamer pack --target macos-arm64 --target windows-x64
dreamer pack --target all            # linux/macos/windows × x64/arm64
```

Cross-compiling non-host triples requires `rustup target add <triple>` and a working linker; failures
are reported (targets are never silently skipped). Pack needs the Dream workspace (set `DREAM_REPO`
if the compiler binary is installed outside a checkout). Libraries cannot be packed.

### How `dreamer run` picks a host

| `package.targets` | No `--target` | With `--target X` |
|---|---|---|
| empty / omitted | **native** (`dream run`) | any of `native` / `web` / `node` (ad-hoc escape hatch) |
| exactly one entry | that host | must match that host |
| two or more | error — require `--target` | `X` must be one of the listed targets |

Per host:

- **native** — `dream run [--release] <entry> [args…]` (wasmtime).
- **node** — compile with `--runtime --node` (refreshing `target/node/`), then `node run.mjs`.
- **web** — compile with `--runtime --web` (refreshing `target/web/`), then serve the project root on
  `http://127.0.0.1:8787/index.html` by default (colored log; Ctrl-C to stop). A later
  `dreamer run --target web` restarts that server on the same port. Override with `--port`.

Use `dreamer run --release` (optionally with `--target`) so release artifacts feed the same stable
alias paths the scaffolds already reference.

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

# publish this project itself to the default GitHub registry
dreamer publish
# or: export DREAM_REGISTRY_TOKEN=ghp_... && dreamer publish --registry https://raw.githubusercontent.com/sps014/dream-registry/main
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
