# Callbacks

Functions cross the Dream/JavaScript boundary in both directions. A Dream function value
(`fun(params): ret`) is an index into the module's function table, which the runtime bridges so JS
can call into Dream and Dream can call into JS.

## Dream → JS

Pass a Dream `fun(...)` to an `extern` whose parameter is a function type. The runtime wraps the
function index as a real JS callable, so the host can invoke it directly:

```dream
fun on_tick(n: int): void {
    println("tick " + n);
}

extern fun run_callback(cb: fun(int): void, times: int): void;

fun main(): void {
    run_callback(on_tick, 3);   // on_tick is passed as a funcref handle
}
```

Host side:

```js
await run("callbacks.wasm", {
  imports: {
    // `cb` arrives already wrapped as a JS callable.
    run_callback: (cb, times) => {
      for (let i = 0; i < times; i++) cb(i);
    },
  },
});
```

The compiler exports the function table as `__indirect_function_table`, and the `*.abi.json` marks `fun(...)` parameters so the runtime knows to wrap the incoming index.

### Registering DOM handlers

Passing a Dream `fun(js): void` / `fun(): void` directly to a dynamic [`js`](references.md) call
auto-wraps it as a JS callable, so event handlers read natively:

```dream
fun on_click(ev: js): void {
    println("clicked: " + ev.type.to_str());
}

fun main(): void {
    let el = js.global("document").getElementById("app");
    el.addEventListener("click", on_click);
}
```

The wrapper has **stable identity per function** - the runtime caches it by funcref table index - so
the same handler registered with `addEventListener` can be removed with `removeEventListener`. When
you need an explicit `js` callable value, use `js.func(handler)` / `js.func0(handler)`.

## JS → Dream

A JavaScript function handed to Dream is just a [`js`](references.md) value; call it directly with
native syntax. Arguments auto-marshal, so no manual boxing is needed:

```dream
fun main(): void {
    let logger = js.global("logger");          // a JS function on the global scope
    logger("hello from Dream");
}
```

```js
globalThis.logger = (msg) => console.log("[logger]", msg);
await run("callbacks.wasm");
```

## Marshaling

Callback arguments and results are marshaled with the same rules as ordinary externs (see [JS Interop](interop.md#value-marshaling)): primitives and `string` convert automatically, and JS values travel as `js` handles.

A complete runnable example lives in [`sample/interop/callbacks.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/callbacks.dream) with its Node runner `callbacks.mjs`.
