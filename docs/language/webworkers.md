# WebWorkers

**Package:** `system.core` (bootstrap — no import required for `WebWorker` / `WebWorkerPool`). Console examples below also use `import system;`.

Dream's [`async`/`await`](async.md) is a *single-threaded* scheduler: tasks interleave at `await` points but never run at the same instant. When you need more than one core — CPU-bound work or parallel pipelines — use a **`WebWorker`**.

A `WebWorker<TIn, TOut>` runs a `fun(TIn): TOut` body on its own OS thread (native) or Web Worker (browser). Each worker has its own private globals, but **shares the same heap memory** with the owner. Heap objects (`@shared class`, `Lock` / `Semaphore`) are visible across workers — real parallelism with shared state, guarded by `@shared` / `lock`.

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
import system;

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

Each `dispatch(msg, body)` call may supply a **different** `body` on the same underlying thread. Overlapping `dispatch` calls to distinct pool members run in parallel exactly like separate `WebWorker`s; `body` capture rules are identical, and `body` may likewise be an async top-level function (see [Async worker bodies](#async-worker-bodies)).

`dispatch` is generic in `TIn`/`TOut` like `WebWorker` — messages are wire-encoded with `Bytes.toWire`/`Bytes.fromWire` under the hood:

```dream
fun square(n: int): int { return n * n; }

async fun main(): void {
    let pool = WebWorkerPool(2);
    let a = pool.dispatch(3, square);
    System.println((await a).to_string()); // 9
    pool.shutdown();
}
```

For an async body (`fun(TIn): Future<TOut>`), use `dispatch_async` instead (two generic overloads that differ only in the body fun type need distinct names). `TIn`/`TOut` follow the same wire constraints as `WebWorker`.

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

A worker body may `await` (including `Time.sleep`, HTTP, or another `async fun`):

- a **named top-level `async fun`**, or an **`async (params) => …` lambda**, typed as `fun(TIn): Future<TOut>`, or
- a **string-returning top-level `async fun`** where `fun(string): string` is expected.

```dream
async fun fetchAndSummarize(url: string): string {
    let resp = await Http.get(url);
    return url + ": " + resp.status.to_string();
}

async fun main(): void {
    let w = WebWorker<string, string>(fetchAndSummarize);
    System.println(await w.send("https://example.com"));
    w.terminate();

    let squarer = WebWorker<int, int>(async (n) => {
        await Time.sleep(1);
        return n * n;
    });
    System.println((await squarer.send(6)).to_string()); // 36
    squarer.terminate();
}
```

This works with `WebWorker.map` and `WebWorkerPool.dispatch` / `dispatch_async` too — each worker runs its body's awaits independently.

## Structured messages

Messages are strings. For structured payloads, serialize at the call site with the [`Json`](../stdlib/json.md) helpers (any `@json` class/struct or `string`):

```dream
@json
class Task { public id: int; public payload: string; }

fun handle(msg: string): string {
    let t = Json.deserialize<Task>(msg).unwrap_or(Task());
    return "done:" + t.id.to_string();
}

async fun main(): void {
    let w = WebWorker<string, string>(handle);
    let t = Task();
    t.id = 7;
    t.payload = "hi";
    System.println(await w.send(Json.serialize(t)));   // done:7
    w.terminate();
}
```

## Runtimes

| Runtime | Notes |
|---------|--------|
| Native (`dream run`) | One OS thread per worker; shared heap with the owner. |
| Browser | One `Worker` per worker; shared memory needs COOP/COEP on the host page. |
| Node | One `worker_threads.Worker` per worker; same shared-memory model. |

## Notes and limits

- The body is `fun(TIn): TOut` or `fun(TIn): Future<TOut>` — a top-level function or (possibly `async`) lambda. See [Sharing state safely](#sharing-state-safely) for capture rules.
- `TIn` / `TOut` must be `string`, an unmanaged value type, or a `T[]` of one. Messages are copied — keep them small. Pass `@shared` state by capturing it in the body lambda, not as the message type.
- A body may `await` — see [Async worker bodies](#async-worker-bodies) and [Functions](functions.md#async-lambdas).
- `terminate()` is idempotent and also runs when the handle is destroyed.

## See also

- [Lock & Semaphore](../stdlib/sync.md) — standalone synchronization primitives for cases not tied to one `@shared` object.
- [Classes & Structs](classes-structs.md) — `@shared class` and the closed-graph field rule.
- [Memory Management](memory.md) — how ARC/refcounting works, including the atomic path `@shared` classes use.
