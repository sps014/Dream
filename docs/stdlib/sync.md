# Lock & Semaphore

**Package:** `system.core` (bootstrap — no import required)

Standalone synchronization primitives for coordinating [`WebWorker`](../language/webworkers.md) threads over shared state. Both are `@shared class` themselves, so they can be captured directly by a `WebWorker` body like any other shared object — see [Sharing state safely](../language/webworkers.md#sharing-state-safely). Console snippets also need `import system;`.

Prefer the built-in `lock (obj) { ... }` statement directly on an `@shared class` instance when you can — it needs no separate `Lock` object and cannot be left unreleased. Reach for a standalone `Lock`/`Semaphore` when the critical section doesn't map to a single block (e.g. it spans a `WebWorker` closure boundary) or you need semaphore-style counting rather than mutual exclusion.

## `Lock`

A reentrant mutual-exclusion lock, equivalent to `lock (obj) { ... }` bracketed manually.

#### `acquire(): void` / `release(): void`

- **Reentrant**: the same thread may call `acquire()` again before releasing — it must call `release()` the same number of times to fully release it. A different thread blocks until the holder releases every level.
- **Blocking**: `acquire()` busy-waits (parks the OS thread on a WASM atomic wait, not a spin loop) until the lock is free.

```dream
@shared
class Account {
    public balance: int;
    public constructor() { this.balance = 0; }
}

fun transfer(acct: Account, amount: int, mtx: Lock): void {
    mtx.acquire();
    acct.balance = acct.balance + amount;
    mtx.release();
}

async fun main(): void {
    let acct = Account();
    let mtx = Lock();

    let w1 = WebWorker<string, string>((_) => { transfer(acct, 10, mtx); return ""; });
    let w2 = WebWorker<string, string>((_) => { transfer(acct, 5, mtx); return ""; });

    await w1.send("");
    await w2.send("");
    System.println(acct.balance);   // 15
}
```

!!! note
    This is exactly what `lock (acct) { acct.balance = acct.balance + amount; }` gives you for free when the critical section is a single object and a single block — reach for a standalone `Lock` only when it isn't.

## `Semaphore`

A classic counting semaphore. `initial` permits are available up front; unlike `Lock`, a semaphore has no notion of ownership — any thread may `release()`, and the same thread may hold more than one permit at once.

#### `Semaphore(initial: int)` / `acquire(): void` / `release(): void`

Use it to cap concurrency — e.g. limiting how many workers touch a resource (a connection pool, a fixed-size buffer) at once:

```dream
async fun main(): void {
    let gate = Semaphore(2);   // at most 2 concurrent holders

    let w1 = WebWorker<string, string>((_) => { gate.acquire(); /* ... */ gate.release(); return ""; });
    let w2 = WebWorker<string, string>((_) => { gate.acquire(); /* ... */ gate.release(); return ""; });
    let w3 = WebWorker<string, string>((_) => { gate.acquire(); /* ... */ gate.release(); return ""; });

    await w1.send("");
    await w2.send("");
    await w3.send("");
}
```

## `CancellationSource` / `CancellationToken`

Cooperative cancellation for long-running work (e.g. [`HttpClient.with_cancellation`](http.md)).

#### `CancellationSource()` / `cancel()` / `is_cancelled()` / `token()`

Creates a cancellation source, signals cancellation to all linked tokens, and exposes a shareable `CancellationToken`. One source can fan out to many consumers (HTTP client, worker loops, etc.).

```dream
let src = CancellationSource();
let token = src.token();
System.println(token.is_cancelled());  // false
src.cancel();
System.println(token.is_cancelled());  // true
System.println(src.is_cancelled());    // true
```

#### `CancellationToken.is_cancelled(): bool`

Polls whether cancellation was requested on this token. Check periodically in long loops — cancellation is cooperative, not preemptive.

```dream
if (token.is_cancelled()) {
    return;
}
```

`CancelledError` implements [`Error`](option-result.md) when a cancelled operation surfaces as `Err`.

## Notes and limits

- Both `Lock` and `Semaphore` are `@shared class` instances and only provide cross-thread synchronization when workers share memory with the owner (native, or browser with COOP/COEP — see [WebWorkers](../language/webworkers.md)).
- `acquire()` busy-waits via a WASM atomic wait, not a spin loop burning CPU — but there is still no fairness guarantee (no FIFO ordering among waiters) and no timeout; a deadlocked pair of workers will wait forever.
- `release()` on a `Lock` you don't hold, or on a `Semaphore` past its `initial` count, is undefined — these primitives trust the caller, the same way C#'s `Monitor`/`SemaphoreSlim` do.
