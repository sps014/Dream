# WebWorkers

Dream's [`async`/`await`](async.md) is a *single-threaded* cooperative scheduler: tasks interleave at
`await` points but never run at the same instant. When you need to use more than one core - CPU-bound
work, parallel pipelines - use a **`WebWorker`**.

A `WebWorker` runs a function on its **own thread** (native, via a fresh `wasmtime` instance) or its
**own Web Worker** (browser/Node-in-browser). Each worker instantiates its *own copy* of the module,
so it has a **completely separate linear memory and fresh globals** - nothing is shared. This is real
OS-level parallelism, not cooperative interleaving.

## The model

```
 Owner instance                     Worker instance (separate heap)
 ─────────────                      ──────────────────────────────
 w.post(msg) ───── copy string ───▶ body(msg): reply
 await w.receive() ◀── copy string ─ reply
```

- **Data is copied, never shared.** Every message crosses the boundary serialized as a string. There
  are no shared object references and no `SharedArrayBuffer`/atomics.
- **The worker body is a top-level `fun(string): string`.** Dream functions capture no state, and the
  function's table index is identical across every instance of the same module, so it is portable to
  the worker.
- **The worker starts clean.** It cannot see the spawner's heap or globals; everything arrives via
  messages.

## API

```dream
public class WebWorker {
    public constructor(body: fun(string): string);

    public fun post(msg: string): void;          // enqueue a message (non-blocking)
    public async fun receive(): string;           // await the next reply (FIFO)
    public async fun send(msg: string): string;   // post + await the matching reply

    public fun terminate(): void;                 // stop the worker (also runs on destruction)
}
```

## Request / response

The simplest use is a strongly-ordered RPC: `send` posts a message and awaits the matching reply.

```dream
fun greet(name: string): string {
    return "hello, " + name + "!";
}

async fun main(): void {
    let w = WebWorker(greet);
    System.println(await w.send("dream"));   // hello, dream!
    w.terminate();
}
```

Replies pair with sends by FIFO order (a worker processes one message at a time), so there is no
correlation-id boilerplate.

## Running work in parallel

To actually use multiple cores, **post to several workers first, then collect** - the workers compute
concurrently while you wait:

```dream
fun work(input: string): string {
    let i = 0;
    while (i < 5000000) { i = i + 1; }   // CPU-bound
    return input.to_upper();
}

async fun main(): void {
    let w1 = WebWorker(work);
    let w2 = WebWorker(work);
    let w3 = WebWorker(work);

    w1.post("alpha");
    w2.post("beta");
    w3.post("gamma");                     // all three now run in parallel

    System.println(await w1.receive());   // ALPHA
    System.println(await w2.receive());   // BETA
    System.println(await w3.receive());   // GAMMA

    w1.terminate();
    w2.terminate();
    w3.terminate();
}
```

`await w1.send(x)` on its own is fully sequential (it waits for the reply before returning); the
`post`-then-`receive` pattern above is what overlaps the work.

## Structured messages

Messages are strings. For structured payloads, serialize at the call site with the [`JSON`](../stdlib/json.md)
helpers (any `@json` class/struct or `string`):

```dream
@json
class Task { public id: int; public payload: string; }

fun handle(msg: string): string {
    let t = JSON.deserialize<Task>(msg);
    // ... work on t ...
    return "done:" + t.id.to_string();
}

async fun main(): void {
    let w = WebWorker(handle);
    let t = Task();
    t.id = 7;
    t.payload = "hi";
    System.println(await w.send(JSON.serialize(t)));   // done:7
    w.terminate();
}
```

## Runtimes

| Runtime | Backing |
|---------|---------|
| Native (`dream run`, `wasmtime`) | One OS thread per worker, each with a fresh `Store`/`Instance` of the same module and a pair of `mpsc` channels. Fully supported and tested. |
| Browser (`runtime/dream.js`) | One `Worker` per worker; the bootstrap re-instantiates the same `.wasm` and exchanges `postMessage` strings. |
| Node (`runtime/dream.js`) | Not supported directly - use the native runtime for parallel workers under Node/CLI. |

Under the hood the module exports a tiny trampoline, `__dream_worker_invoke(fn_idx, msg_ptr)`, which
performs one `call_indirect` on the body funcref; the host worker driver calls it once per message.
`receive()`/`send()` are `extern async`, so they bridge into the normal async scheduler exactly like
an HTTP request: native pre-resolves the future once the reply lands, and the browser resolves it from
the worker's `onmessage`.

## Notes & limits

- The worker body must be a **top-level `fun`** with signature `fun(string): string`.
- Workers do not share memory; large payloads are copied. Keep messages reasonably small or chunk them.
- A worker body should be **synchronous** (no `await` inside the body).
- `terminate()` is idempotent and also runs automatically when the `WebWorker` handle is destroyed.
