# WebWorkers

Dream's [`async`/`await`](async.md) is a *single-threaded* scheduler: tasks interleave at `await` points but never run at the same instant. When you need more than one core — CPU-bound work or parallel pipelines — use a **`WebWorker`**.

A `WebWorker<TIn, TOut>` runs a `fun(TIn): TOut` body on its own OS thread (native, via a fresh `wasmtime` instance) or its own Web Worker (browser). Each worker instantiates its *own copy* of the module, so it has its own private globals and shadow stack — but on native, every worker instance **shares the same linear memory** as the owner. That means anything already on the heap (an `@shared class` instance, a `Lock`/`Semaphore`, a plain unmanaged value) is visible to every worker, not copied — this is real parallelism with real shared state, guarded explicitly by `@shared`/`lock` rather than implicitly.

`TIn`/`TOut` must each be `string`, an `unmanaged` (blittable) value type, or a `T[]` array of one (e.g. `int[]`, `Point[]`): every message crosses the thread boundary on an internal wire format (`string` as-is, an unmanaged value as a raw byte-blit, an array as a dynamic-length raw byte-blit of its elements) — never a live pointer into memory the other thread might mutate. An array always crosses as an independent copy, exactly like a scalar/struct value; the two sides never end up aliasing the same heap block.

!!! note "Browser status"
    The browser runtime (`runtime/dream.js`) now imports the same shared `WebAssembly.Memory` (`SharedArrayBuffer`) into every spawned `Worker`, matching native — but the host page must be served with the [Cross-Origin Isolation](https://developer.mozilla.org/en-US/docs/Web/API/crossOriginIsolated) headers (`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`, or `credentialless`) or `SharedArrayBuffer` allocation fails silently in some browsers. Plain message-passing (`post`/`receive`/`send`) works without those headers; `@shared` capture across workers does not.

## The model

```
 Owner instance                            Worker instance
 ─────────────                             ───────────────
 w.post(msg) ──────── copy string ───────▶ body(msg): reply
 await w.receive() ◀── copy string ─────── reply

              (native) same shared linear memory underneath —
              an @shared object mutated by one thread is visible to the other
```

- **The worker body is a `fun(TIn): TOut` value** — a top-level function *or a lambda*. Its function-table index is identical across every instance of the module, so it is portable to the worker. A body is usually synchronous, but a non-capturing top-level `async fun` returning `string` is also allowed — see [Async worker bodies](#async-worker-bodies).
- **A capturing lambda body may only capture `@shared class` instances or unmanaged/value locals.** Capturing an ordinary managed object reference is a compile-time error — see [Sharing state safely](#sharing-state-safely) below.

## API

```dream
public class WebWorker<TIn, TOut> {
    public constructor(body: fun(TIn): TOut);

    public fun post(msg: TIn): void;              // enqueue a message (non-blocking)
    public async fun receive(): TOut;             // await the next reply (FIFO)
    public async fun send(msg: TIn): TOut;        // post + await the matching reply

    public static async fun map(items: TIn[], body: fun(TIn): TOut): TOut[];

    public fun terminate(): void;                 // stop the worker (also runs on destruction)
}
```

## Request / response

The simplest use is a strongly-ordered RPC — `send` posts and awaits the matching reply. Replies pair with sends by FIFO order (one message at a time), so there is no correlation-id boilerplate:

```dream
fun greet(name: string): string {
    return "hello, " + name + "!";
}

async fun main(): void {
    let w = WebWorker<string, string>(greet);
    System.println(await w.send("dream"));   // hello, dream!
    w.terminate();

    // A value-typed channel: TIn/TOut cross as a raw byte-blit, no serialization to write.
    let squarer = WebWorker<int, int>((n) => n * n);
    System.println((await squarer.send(6)).to_string()); // 36
}
```

## Running work in parallel

To actually use multiple cores, **post to several workers first, then collect** — they compute concurrently while you wait:

```dream
fun work(input: string): string {
    let i = 0;
    while (i < 5000000) { i = i + 1; }   // CPU-bound
    return input.to_upper();
}

async fun main(): void {
    let w1 = WebWorker<string, string>(work);
    let w2 = WebWorker<string, string>(work);
    let w3 = WebWorker<string, string>(work);

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

`await w1.send(x)` alone is fully sequential (it waits for the reply); the `post`-then-`receive` pattern above is what overlaps the work.

For the common "run N independent computations and collect the results" shape, `WebWorker.map` (below) does this for you without the manual `post`/`receive` bookkeeping.

## `WebWorker.map` — parallel map

`WebWorker<TIn, TOut>.map` fans a body out over an array in parallel and collects replies **in input order** — one worker per element, spawn-then-collect (not spawn-then-immediately-await):

```dream
fun square(x: string): string {
    let n = int.parse(x).unwrap_or(0);
    return (n * n).to_string();
}

async fun main(): void {
    let items = ["1", "2", "3", "4", "5"];
    let results = await WebWorker<string, string>.map(items, square);
    for (let r in results) {
        System.println(r);   // 1, 4, 9, 16, 25
    }
}
```

The `body` argument follows the same capture rules as the plain `WebWorker` constructor. A lambda passed here is checked the same way — only `@shared class` instances and unmanaged/value locals may be captured. `body` may also be an async top-level function (see [Async worker bodies](#async-worker-bodies)).

## `WebWorkerPool` — reuse worker threads

For many short jobs over time, spawning a fresh OS thread per task is wasteful. A `WebWorkerPool` keeps a fixed set of worker threads alive and **dispatches** work to them round-robin:

```dream
async fun main(): void {
    let pool = WebWorkerPool(4);
    let a = pool.dispatch("3", square);
    let b = pool.dispatch("4", square);
    let c = pool.dispatch("5", square);
    System.println(await a);   // 9
    System.println(await b);   // 16
    System.println(await c);   // 25
    pool.shutdown();
}
```

Each `dispatch(msg, body)` call may supply a **different** `body` on the same underlying thread — the pool member runs whichever `(funcidx, env)` pair you pass for that call. Overlapping `dispatch` calls to distinct pool members run in parallel exactly like separate `WebWorker`s; `body` capture rules are identical, and `body` may likewise be an async top-level function returning `string` (see [Async worker bodies](#async-worker-bodies)).

Instance `dispatch` is `string`-in/`string`-out (instance methods aren't monomorphized per call-site type arguments). For typed messages, use the free-function helpers:

```dream
fun square(n: int): int { return n * n; }

async fun main(): void {
    let pool = WebWorkerPool(2);
    let a = pool_dispatch<int, int>(pool, 3, square);
    System.println((await a).to_string()); // 9
    pool.shutdown();
}
```

`pool_dispatch` wraps with `Bytes.toWire`/`Bytes.fromWire` the same way `WebWorker<TIn, TOut>` does. For an async body (`fun(TIn): Future<TOut>`), use `pool_dispatch_async` instead. `TIn`/`TOut` follow the same wire constraints as `WebWorker`.

Pick a pool when you're issuing many small tasks over time; pick `WebWorker`/`WebWorker.map` for a single batch of parallel work you want to spawn, await, and tear down together.

## Sharing state safely

A worker body may be a **capturing lambda**, not just a top-level function — as long as everything it captures is safe to touch from another thread:

- an **`@shared class`** instance (see [Classes & Structs](classes-structs.md)), or
- an **unmanaged/value** local (a primitive or a value `struct`, snapshotted by value).

Capturing an ordinary managed object reference is a compile-time error: it would hand the worker a live pointer into memory the creating thread may keep mutating non-atomically, silently racing.

```dream
@shared
class Counter {
    public value: int;
    constructor() { this.value = 0; }

    public fun increment(): void {
        lock (this) {
            this.value = this.value + 1;
        }
    }
}

async fun main(): void {
    let counter = Counter();

    let a = WebWorker<string, string>((msg) => { counter.increment(); return msg; });
    let b = WebWorker<string, string>((msg) => { counter.increment(); return msg; });

    await a.send("go");
    await b.send("go");

    System.println(counter.value);   // 2
    a.terminate();
    b.terminate();
}
```

`lock (obj) { ... }` is a reentrant mutual-exclusion block — the direct equivalent of C#'s `lock`. It works on any `@shared class` instance (or a standalone `Lock`/`Semaphore` — see [Lock & Semaphore](../stdlib/sync.md)). The same thread re-entering a `lock` it already holds (e.g. through recursion) bumps a recursion counter instead of deadlocking; a different thread blocks until the lock is free.

An `@shared class`'s fields must themselves be unmanaged/value types or other `@shared class` instances (the "closed-graph" rule) — this keeps every reachable piece of a shared object's state under the same protection, so there is no back door to an unprotected managed reference.

## Async worker bodies

A worker body may `await` — including a real host call like `Time.sleep`, an HTTP request, or another `async fun` — in either of two shapes:

- a **named top-level `async fun`**, or an **`async (params) => …` lambda**, typed as `fun(TIn): Future<TOut>` (the Future-body constructor / `map` / `dispatch` overloads), or
- a **string-returning top-level `async fun`** passed where `fun(string): string` is expected (the original trampoline path: identity `toWire`, string-only).

```dream
async fun fetchAndSummarize(url: string): string {
    let resp = await Http.get(url);
    return url + ": " + resp.status.to_string();
}

async fun main(): void {
    let w = WebWorker<string, string>(fetchAndSummarize);
    System.println(await w.send("https://example.com"));
    w.terminate();

    // Async lambda — same Future-body overload; `TOut` need not be `string`.
    let squarer = WebWorker<int, int>(async (n) => {
        await Time.sleep(1);
        return n * n;
    });
    System.println((await squarer.send(6)).to_string()); // 36
    squarer.terminate();
}
```

This works with `WebWorker.map` and `WebWorkerPool.dispatch` too — each worker drives its own body's awaits to completion independently, so `map`'s parallelism still holds even when every element's work involves a real await.

**Why the Future-body overload exists.** The worker-invoke trampoline distinguishes "the body already finished" from "the body is an async task still running" by checking the raw `call_indirect` result's heap tag: an async constructor returns an untagged `Future` frame pointer. The Future-body overload wraps the user's `fun(TIn): Future<TOut>` in an async `fun(string): Future<string>` that `await`s the body and then `Bytes.toWire`s the result — so the trampoline always unwraps a settled wire `string`, and any unmanaged `TOut` is sound. The older string-only path (boxing a top-level `async fun` as `fun(string): string` and relying on identity `toWire`) remains for compatibility with existing string async bodies.

**Native vs. browser.** On native (`wasmtime`), every host `async` op resolves synchronously before `call_indirect` returns, so driving the task to completion is a single, immediate pass. In the browser, a real `extern async` host call (a genuine `fetch`, a real-time timer) instead settles later via a JS Promise callback — `runtime/dream.js`'s worker driver accounts for this by polling the `Future`'s status on the macrotask queue (not synchronously) until some pending Promise resolves it, rather than assuming one pass is enough. Both backends produce the same result; the browser path just genuinely waits for real time to pass instead of finishing in one call.

## Structured messages

Messages are strings. For structured payloads, serialize at the call site with the [`JSON`](../stdlib/json.md) helpers (any `@json` class/struct or `string`):

```dream
@json
class Task { public id: int; public payload: string; }

fun handle(msg: string): string {
    let t = JSON.deserialize<Task>(msg);
    return "done:" + t.id.to_string();
}

async fun main(): void {
    let w = WebWorker<string, string>(handle);
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
| Native (`dream run`, `wasmtime`) | One OS thread per worker, each with a fresh `Store`/`Instance` importing the *same* `wasmtime::SharedMemory` as the owner, plus a pair of `mpsc` channels for messages. Fully supported and tested. |
| Browser (`runtime/dream.js`) | One `Worker` per worker, each importing the parent's shared `WebAssembly.Memory` (requires COOP/COEP on the host page). Messages still cross as copied strings; `@shared` heap objects are visible across workers when isolation headers are set. |
| Node (`runtime/dream.js`) | One `worker_threads.Worker` per worker, each importing the parent's shared `WebAssembly.Memory` (SharedArrayBuffer is available under `worker_threads`). Same message/`@shared` model as the browser path. |

Under the hood the module exports a trampoline, `__dream_worker_invoke(fn_idx, env, msg_ptr)`, that publishes `env` (the body's closure environment word — 0 for a non-capturing body, an `@shared`-object pointer or a snapshotted unmanaged environment otherwise) to the closure-env global, then performs one `call_indirect` on the body funcref — and, if that call turns out to have hit an `async fun`'s constructor rather than an ordinary function, drives it to completion and unwraps the real result before returning (see [Async worker bodies](#async-worker-bodies)). The host driver calls it once per message, reusing the same `(fn_idx, env)` pair for every message a given worker ever processes. `receive()`/`send()` are `extern async`, bridging into the normal async scheduler like an HTTP request. The browser/Node `dream.js` path calls a lower-level `__dream_worker_invoke_raw` export instead and drives async completion itself (see above), since it cannot assume a pending task settles synchronously.

## Notes and limits

- The worker body's type is `fun(TIn): TOut` or `fun(TIn): Future<TOut>` — a top-level function or a (possibly `async`) lambda (see [Sharing state safely](#sharing-state-safely) for what a lambda may capture).
- `TIn`/`TOut` must be `string`, an `unmanaged` (blittable) value type, or a `T[]` of one; a message is always a copy across the wire (never a shared array/struct pointer), so keep it small or chunk large payloads. `@shared class` is deliberately not supported here — capture it in the body lambda instead (see [Sharing state safely](#sharing-state-safely)).
- A worker body may `await` (see [Async worker bodies](#async-worker-bodies)) via the `fun(...): Future<T>` overloads (named `async fun` or `async` lambda, any valid `TOut`) or via a string-returning top-level `async fun` on the legacy `fun(string): string` path. See [Functions](functions.md#async-lambdas) for async lambda syntax.
- `terminate()` is idempotent and also runs automatically when the handle is destroyed.
- The coarse allocator lock (guarding every `malloc`/`free` on the shared heap, not just `@shared`-object access) serializes heap-touching operations across all workers under heavy concurrent allocation — a known v1 limitation, not a correctness issue.

## See also

- [Lock & Semaphore](../stdlib/sync.md) — standalone synchronization primitives for cases not tied to one `@shared` object.
- [Classes & Structs](classes-structs.md) — `@shared class` and the closed-graph field rule.
- [Memory Management](memory.md) — how ARC/refcounting works, including the atomic path `@shared` classes use.
