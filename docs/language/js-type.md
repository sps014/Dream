# The `js` type

`js` is a handle to a live JavaScript value — a DOM node, a `fetch` `Response`, a `RegExp`, a
plain object, or a function. You read, call, and mutate it with **ordinary Dream syntax**.

```dream
fun main(): void {
    let doc = js.global.document;
    let el = doc.getElementById("app");
    el.textContent = "hello";
    el.classList.add("a", "b", "c");

    let n: int = el.childNodes.length;
    println("children: " + n);
}
```

## How `js` values work

`js` is a real static type, but the compiler does **no member resolution** on it. Any `.name`,
`.name(...)`, `[key]`, or call type-checks; whether the member exists is decided at runtime by the
JS host. Dynamic operations return `js`, so chains like `el.classList.add(...)` just work.

You leave the dynamic world at a **typed boundary** — assigning to a typed variable, passing a typed
argument, or returning a typed value — where Dream converts automatically:

```dream
let count: int = config.count;   // js -> int here
```

A `js` value is not a Dream heap object, so it is never reference-counted. Dream will not free it
for you.

!!! warning "Release long-lived handles"
    Call `.release()` when you are done with a handle you hold for a while. Short-lived handles
    used within a function and then dropped do not need this.

## Getting a `js` value

| Entry point | Gives you |
| --- | --- |
| `js.global` | `globalThis` — e.g. `js.global.document`, `js.global.fetch(...)` |
| `js.global(name)` | `globalThis[name]`, for a runtime-only name |
| `js.object()` | a fresh empty `{}` |
| `js.array()` | a fresh empty `[]` |
| `js.func(handler)` / `js.func0(handler)` | wrap a Dream function as a JS callable — see [Callbacks](callbacks.md) |

```dream
let opts = js.object();
opts.method = "POST";
opts.keepalive = true;
js.global.fetch("/api", opts);
```

## Reading, writing, and calling

```dream
let el = js.global.document.getElementById("app");

let cls: js = el.className;
el.className = "highlighted";
el.classList.add("a", "b", "c");
let first = el.children[0];
el.children[0] = replacement;
```

## Passing values to JS

| Dream value | Crosses as |
| --- | --- |
| `int` / `long` / `double` / `bool` / `string` | itself |
| another `js` | passed through |
| an array of the above (`int[]`, `string[]`, `js[]`, …) | a JS array |
| a Dream function (`fun(...)`) | a JS callable — see [Callbacks](callbacks.md) |
| a `struct` / `class` | a **deep copy** into a plain JS object |

A `union` or `List<T>` is not marshalable directly — convert to an array or struct field first.

### Structs and classes

Passing a `struct` or `class` to JS deep-copies it into a plain object (nested fields and arrays
included). Fields that cannot cross (maps, interfaces, function values) are left off:

```dream
class Point {
    public x: int;
    public y: int;
    public constructor(x: int, y: int) { this.x = x; this.y = y; }
}

js.global.render(Point(3, 4));   // -> render({ x: 3, y: 4 })
```

Assigning a `js` object to a `class`-typed variable rebuilds an instance by reading each declared
field — the constructor is not called:

```dream
let p: Point = js.global.originPoint();
```

!!! note
    Reconstruction targets reference `class` types; a value `struct` return is not supported yet.

## Getting values back out

A result from JS stays a `js` value until a typed binding, argument, or return converts it. Or
convert explicitly:

| Method | Converts to |
| --- | --- |
| `to_int()` / `to_double()` / `to_bool()` / `to_str()` | the matching Dream primitive |
| `is_null()` | `true` if the value is `null` or `undefined` |
| `release()` | drops the host-side handle |

## Awaiting JS Promises

Await a JS Promise directly — it resolves to **`Option<js>`**: `Some(value)` on success, `None`
when the Promise rejected or resolved with `null`/`undefined`:

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

`await` may appear anywhere in an `async` function, including loops and branches.

For a typed extern that returns a Promise (`@js(...) extern async fun getUser(...): string`), see
[Async/Await](async.md#awaiting-javascript-promises).

## Where it runs

`js` needs a JavaScript host (browser or Node). The standalone native runner stubs JS imports as
traps.

## Try it

- [`sample/interop/js.dream`](https://github.com/sps014/dream/blob/main/sample/interop/js.dream)
- [`sample/interop/slots.dream`](https://github.com/sps014/dream/blob/main/sample/interop/slots.dream)
- [`sample/interop/structs.dream`](https://github.com/sps014/dream/blob/main/sample/interop/structs.dream)
- [`sample/interop/async_js.dream`](https://github.com/sps014/dream/blob/main/sample/interop/async_js.dream)
