# Callbacks

Functions cross the Dream/JavaScript boundary in both directions: hand a Dream function to JS, or
call a JS function from Dream. A Dream function value (`fun(params): ret`) is an index into the
module's function table, and the runtime wraps that index as a real, callable JS function.

## Dream → JS

Declare an `extern` parameter with a function type, and pass a Dream function where it's expected:

```dream
fun on_tick(n: int): void {
    println("tick " + n);
}

extern fun run_callback(cb: fun(int): void, times: int): void;

fun main(): void {
    run_callback(on_tick, 3);   // on_tick crosses as a JS-callable wrapper
}
```

The host receives `cb` already wrapped and just calls it like any other JS function:

```js
await run("callbacks.wasm", {
  imports: {
    run_callback: (cb, times) => {
      for (let i = 0; i < times; i++) cb(i);
    },
  },
});
```

Behind the scenes, the compiler exports the function table as `__indirect_function_table`, and the
generated `*.abi.json` marks `fun(...)` parameters so the runtime knows to wrap the incoming index
before the host ever sees it.

## Registering DOM handlers

Passing a Dream function directly into a dynamic [`js`](references.md) call wraps it automatically
— so event handlers read exactly like they would in JavaScript:

```dream
fun on_click(ev: js): void {
    println("clicked: " + ev.type.to_str());
}

fun main(): void {
    let el = js.global.document.getElementById("app");
    el.addEventListener("click", on_click);
}
```

The wrapper has **stable identity per function** — the runtime caches it by function-table index —
so a handler registered with `addEventListener` can later be removed with the *same* Dream function
value passed to `removeEventListener`. If you need an explicit `js` callable value up front (to
store, compare, or pass around before registering it), build one with `js.func(handler)` /
`js.func0(handler)`.

## JS → Dream

A JavaScript function handed to Dream is just a [`js`](references.md) value — call it with native
syntax, and its arguments auto-convert on the way in:

```dream
fun main(): void {
    js.global.logger("hello from Dream");
}
```

```js
globalThis.logger = (msg) => console.log("[logger]", msg);
await run("callbacks.wasm");
```

## Marshaling

Callback arguments and results follow the same conversion rules as ordinary externs and dynamic
`js` calls (see [Value marshaling](interop.md#value-marshaling) and
[Passing values to JS](references.md#passing-values-to-js)): primitives and `string` convert
automatically, and JS values travel as `js` handles.

## Try it

[`sample/interop/callbacks.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/callbacks.dream)
runs both directions end to end, with its Node runner
[`callbacks.mjs`](https://github.com/sps014/Dream/blob/main/sample/interop/callbacks.mjs).
