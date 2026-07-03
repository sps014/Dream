# The `js` type

`js` is Dream's dynamic JavaScript-interop type: a handle to a live JavaScript value. A real JS
object — a DOM node, a `fetch` `Response`, a `RegExp`, a plain object, a function — crosses into
Dream as a `js` value and you read, call, and mutate it with **native syntax**, no fixed-arity
helpers or string-keyed lookups required.

```dream
fun main(): void {
    let doc = js.global.document;
    let el = doc.getElementById("app");
    el.textContent = "hello";
    el.classList.add("a", "b", "c");

    let n: int = el.childNodes.length;   // js -> int at the typed binding
    println("children: " + n);
}
```

Every one of those operations — the property read, the method call with three arguments, the
nested property access — is ordinary Dream syntax. There is no special API to learn: if you know
how to write it in JavaScript, you know how to write it in Dream.

## How `js` values work

A `js` value behaves like C# `dynamic` or TypeScript `any`: it's a real static type, but the
compiler does **no member resolution** on it. Any `.name`, `.name(...)`, `[key]`, or call on a
`js` value type-checks unconditionally and compiles to a runtime bridge call — whether the member
actually exists is decided later, by the JS host itself. Because every dynamic operation returns
another `js`, chains like `el.classList.add(...)` type-check with no extra work on your part.

You leave the dynamic world only at a **typed boundary** — assigning to a typed variable, passing
a typed argument, or returning a typed value — where the compiler inserts an automatic conversion:

```dream
let count: int = config.count;   // js -> int here, automatically
```

Under the hood, a `js` value is just an `i32` id into a handle table the runtime keeps on the JS
side. When a bridge call returns a JS value, the runtime registers it and hands Dream the id; when
that id comes back, the runtime looks the value up again.

```mermaid
flowchart LR
  jsv["JS value (RegExp / Response / fn)"] -->|registerHandle| id["i32 id"]
  id -->|"js in Dream"| dream["Dream code"]
  dream -->|"id passed back"| deref["derefHandle -> JS value"]
```

Because a `js` value isn't a Dream heap object, it's never reference-counted — Dream will not free
it for you.

!!! warning "Release long-lived handles"
    Call `.release()` when you're done with a handle you're holding onto for a while, to drop the
    host-side entry and avoid leaking it. Short-lived handles used within a function and then
    dropped don't need this.

## Getting a `js` value

Every program starts from one of these entry points:

| Entry point | Gives you |
| --- | --- |
| `js.global` | `globalThis` — read off it: `js.global.document`, `js.global.fetch(...)` |
| `js.global(name)` | `globalThis[name]`, for a name you only know at runtime |
| `js.object()` | a fresh empty `{}` to populate and pass to a JS API |
| `js.array()` | a fresh empty `[]` |
| `js.func(handler)` / `js.func0(handler)` | wrap a Dream function as a JS callable — usually implicit, see [Callbacks](callbacks.md) |

```dream
let opts = js.object();
opts.method = "POST";
opts.keepalive = true;
js.global.fetch("/api", opts);
```

## Reading, writing, and calling

Once you have a `js` value, everything else reads exactly like JavaScript:

```dream
let el = js.global.document.getElementById("app");

let cls: js = el.className;         // property read
el.className = "highlighted";       // property write
el.classList.add("a", "b", "c");    // method call, any number of arguments
let first = el.children[0];         // indexing, e.g. array/NodeList access
el.children[0] = replacement;       // index assignment
```

## Passing values to JS

Arguments to a dynamic call or property write auto-convert on their way out — you never box a
value by hand:

| Dream value | Crosses as |
| --- | --- |
| `int` / `long` / `double` / `bool` / `string` | itself, directly |
| another `js` | its handle, passed through |
| an array of any of the above (`int[]`, `string[]`, `js[]`, …) | a JS array |
| a Dream function (`fun(...)`) | a JS callable — see [Callbacks](callbacks.md) |
| a `struct` / `class` | a **deep copy** into a plain JS object — see below |

A `union` or `List<T>` isn't marshalable directly; convert it to an array or a struct field first.

### Structs and classes

Passing a `struct` or `class` value to JS deep-copies it into a plain object, recursing into
nested struct/class fields and arrays. Fields whose type can't cross the boundary (maps,
interfaces, function values) are simply left off the object.

```dream
class Point {
    public x: int;
    public y: int;
    constructor(x: int, y: int) { this.x = x; this.y = y; }
}

js.global.render(Point(3, 4));   // -> render({ x: 3, y: 4 })
```

The conversion also runs in reverse: assigning a `js` object to a `class`-typed variable rebuilds
an instance by reading each declared field straight off the object — the class's constructor is
not called.

```dream
let p: Point = js.global.originPoint();   // reconstructed from { x, y }
```

!!! note
    Reconstruction targets reference `class` types today; a value `struct` return isn't supported
    yet.

## Getting values back out

A result from JS stays a `js` value. It converts to a Dream primitive automatically at a typed
binding, argument, or return — as in the very first example (`let n: int = el.childNodes.length`).
Anywhere else, convert explicitly:

| Method | Converts to |
| --- | --- |
| `to_int()` / `to_double()` / `to_bool()` / `to_str()` | the matching Dream primitive |
| `is_null()` | `true` if the value is `null` or `undefined` |
| `release()` | drops the host-side handle |

## Awaiting JS Promises

A JS Promise is itself just a `js` value, so you `await` it directly — no `extern async fun`
declaration needed. It resolves to an **`Option<js>`**: `Some(value)` when the Promise resolved,
`None` when it rejected (the rejection reason is logged host-side) or resolved with
`null`/`undefined`. Failure is baked into the type, so it can't be silently ignored.

```dream
async fun load(): void {
    let resp = await js.global.fetch("/api");
    switch (resp) {
        Some(r) => {
            let ok: bool = r.ok;
            println("ok = " + ok);
        }
        None => println("request failed"),
    }
}
```

An `await` may appear anywhere in an `async` function, including inside a loop or branch body, so you
can fetch conditionally or in a loop without hoisting:

```dream
for (let url in urls) {
    let resp = await js.global.fetch(url);   // suspends each iteration
    // ... use resp ...
}
```

For a plain typed extern that returns a Promise (`@js(...) extern async fun getUser(...): string`),
see [Async/Await](async.md#awaiting-javascript-promises) instead — that path skips `js` entirely
and unboxes straight to the declared return type.

## How calls are marshaled

A dynamic call's arguments never touch the heap. The compiler writes each argument into a
16-byte **tagged slot** (a type tag plus the value or a handle) on the shadow stack, then hands the
JS host a pointer and a count in a single boundary crossing — no per-argument boxing, no
intermediate array. The buffer is released the instant the call returns. This keeps a dynamic call
close to the cost of a native one, even with several mixed-type arguments:

```dream
api.log("row", 42, true, 3.5);   // string + int + bool + double, one crossing
```

## Where it runs

`js` is backed by the `Dream` host module in `runtime/dream.js`, so it only works under a real JS
runtime (browser or Node) — not the standalone `wasmtime` test harness, where the interop imports
are stubbed as traps.

## Try it

- [`sample/interop/js.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/js.dream) —
  reading, calling, and mutating a JS object (run with `sample/interop/js.mjs`).
- [`sample/interop/slots.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/slots.dream) —
  mixed-type, array, and callback arguments in one call.
- [`sample/interop/structs.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/structs.dream) —
  passing a class to JS and reconstructing one from a JS object.
- [`sample/interop/async_js.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/async_js.dream) —
  awaiting a `js` Promise.
