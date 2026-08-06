# Async / Await

**Packages:** `Promise` / `Future` are bootstrap (`system.core`). `Time.sleep` needs `import system;`. HTTP examples need `import system.net;`.

Dream has cooperative concurrency with `async`/`await`. The execution model is **eager**, like JavaScript: calling an `async fun` starts the work immediately and hands you a `Future<T>` handle; `await` retrieves the result.

## Declaring and awaiting

Prefix a function with `async`. Its declared return type `T` becomes `Future<T>` at the call site. `await e` suspends the current task until `e`'s future resolves, then yields its value:

```dream
import system;

async fun fetchData(): string {
    await Time.sleep(100);   // suspends this task; the event loop keeps running
    return "data";
}

async fun main(): void {
    let x = await fetchData();   // x : string
    System.println(x);
}
```

`await f()` is just the call composed with `await`: `f()` produces a `Future<T>`, and `await` suspends on it to get `T`. The only rule is that `await` outside an `async` function is an error.

### Where `await` is allowed

`await` may appear in any expression or statement position inside an `async` function — including conditionally evaluated ones:

```dream
let x = await e;                // bind the result
let y = await f() + 1;          // in an operand
process(await a(), await b());  // several awaits in call arguments

if (retry) { data = await fetch(url); }         // in a branch
while (i < n) { sum += await g(i); i += 1; }    // suspends each iteration
let y = cond ? await a() : await b();           // in a ternary arm
let z = flag && await ready();                  // right side of && / || / ??
```

## Running work concurrently

Because calls are eager, you can start several before the first `await` and let them run concurrently, then combine them:

```dream
import system;

async fun work(id: int): int {
    await Time.sleep(50);
    return id * id;
}

async fun main(): void {
    let a = work(2);                         // started now
    let b = work(3);                         // started now
    let results = await Promise.all([a, b]); // both ran concurrently -> [4, 9]
    System.println(results[0] + ", " + results[1]);
}
```

### Combinators (`Promise`)

Static methods on the built-in `Promise` class, over `Future<T>[]`:

| Method | Signature | Resolves when |
| --- | --- | --- |
| `Promise.all` | `Promise.all(xs: Future<T>[]): Future<T[]>` | every future has resolved (results in order) |
| `Promise.any` | `Promise.any(xs: Future<T>[]): Future<T>` | the first future resolves |
| `Promise.race` | `Promise.race(xs: Future<T>[]): Future<T>` | the first future settles |

```dream
let first = await Promise.any([work(10), work(20)]);
```

`Time.sleep(ms: int): Future<void>` is an awaitable timer backed by the runtime's timer queue (a virtual clock natively, `setTimeout` in the browser). It composes with the combinators like any other future.

## Cancellation

Bootstrap types `CancellationSource` / `CancellationToken` / `CancelledError` support cooperative cancellation:

```dream
let src = CancellationSource();
let tok = src.token();
src.cancel();
System.println(tok.check().is_err()); // true → ECANCELLED
```

`Promise.cancel(future)` marks a future cancelled (unlinks pending timers via `$dream_cancel`). Prefer tokens for app-level checks; native in-flight HTTP cancel remains best-effort (`HttpClient.with_cancellation` + `with_timeout`).

## Async methods

Instance and `static` class methods can be `async`, so a type can own its asynchronous behavior. The call types as `Future<T>` just like a free async call:

```dream
import system;
import system.net;

class Downloader {
    url: string;
    async fun fetch(): string {
        let body = await HttpClient("").text(this.url);
        return body;
    }
}

async fun main(): void {
    let d = Downloader("https://example.com");
    let body = await d.fetch();   // d.fetch() : Future<string>
    System.println(body);
}
```

Async methods work on **generic** classes too: each concrete type gets its own async method.

### Async lambdas and `fun(...): Future<T>` values

An `async (params) => …` arrow lambda is typed as `fun(...): Future<T>` — see [Functions](functions.md#async-lambdas). Calling the boxed value returns a `Future` just like calling a named `async fun`; `await` unwraps it. Named async functions used as first-class values (`let f: fun(int): Future<int> = delayed;`) use the same shape.

## Awaiting JavaScript promises

An `extern async fun` bridges to a host function that returns a Promise. Dream source never sees the Promise itself:

```dream
@js("api", "getUser")
extern async fun getUser(id: int): string;

async fun main(): void {
    let name = await getUser(42);
    System.println("user = " + name);
}
```

```js
import { run } from "./dream.js";

await run("user.wasm", {
  imports: {
    getUser: (id) => fetch(`/api/user/${id}`).then((r) => r.text()),
  },
});
```

A complete example: [`sample/interop/async_fetch.dream`](https://github.com/sps014/Dream/blob/main/sample/interop/async_fetch.dream).

## Limitations

- No `.then()` / callback chaining — use `async` / `await`.
- Tasks interleave at `await` points on one thread. For real parallelism, see [WebWorkers](webworkers.md).
