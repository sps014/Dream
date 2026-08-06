# Memory Management

Dream manages heap memory with **Automatic Reference Counting (ARC)**. You never call `free`, and there is no garbage collector to pause your program — memory is reclaimed the moment the last reference to an object drops.

## What lives on the heap

- Strings
- Arrays (`T[]`)
- Class instances
- Standard library collections (`List`, `Map`, `Set`)

Primitives (`int`, `float`, `bool`, ...) and value `struct`s are stored on the stack or inline inside other objects — no heap allocation.

## How it works

Every heap object tracks how many references point to it. The compiler inserts `retain` and `release` for you:

- When a variable goes out of scope, its reference is released.
- Reassigning a variable releases the value it held before.
- When a count reaches zero, the object is freed immediately (its `del` destructor runs first, if it has one).

```dream
fun make_list(): int[] {
    let arr = [1, 2, 3];   // allocated, count = 1
    return arr;            // handed to the caller
}

fun main() {
    let result = make_list();
    println(result[0]);
} // result leaves scope -> count 0 -> freed instantly
```

## Advanced: reference cycles

ARC relies on counts, so it cannot collect a **cycle**. If `A` references `B` and `B` references `A`, neither count ever reaches zero — a leak:

```dream
class Node {
    public next: Option<Node>;
}

let a = Node(...);
let b = Node(...);
a.next = Option.Some(b);
b.next = Option.Some(a);   // cycle created — `a` and `b` now leak
```

### The compiler catches this for you

Rather than relying on you to notice, the compiler builds a graph of every `class`'s strong (non-`weak`/`unowned`) fields and hard-errors on any cycle in it — including a class holding a field of its own type, since that field could always be wired into a self-cycle:

```
error: reference cycle detected: 'Node.next' form a strong-reference cycle, so none of their
objects can ever be freed; mark one field 'weak' or 'unowned' to break it, or annotate every
class in the cycle with '@allow_cycle' if the cycle is intentional
```

This is a **structural**, type-level check, not a value-level one: it flags "these class types are structurally capable of forming a cycle," not "this specific program creates one." It reliably catches the textbook cases — direct self-reference, parent/child, doubly-linked lists, observer/observed — but it can't see cycles assembled dynamically through collections, `object`, or callbacks (e.g. a list that happens to contain itself); those still require programmer discipline.

### Breaking a cycle: `weak` and `unowned`

Mark one side of the cycle `weak` or `unowned` so it doesn't hold a strong reference:

```dream
class Node {
    public next: Option<Node>;
    weak parent: Option<Node>;    // does not keep the parent alive
}

class Cache {
    unowned owner: Manager;       // does not keep `owner` alive
}
```

- **`weak T`** — the field must be `Option<T>` for a class `T`. Read it like any other `Option`: `switch`, `.unwrap_or(...)`, `.is_some()`.
- **`unowned T`** — the field must itself be a class type `T` (not wrapped in `Option`). Use it only when a stronger invariant (e.g. "the parent always outlives the child") already guarantees the referent is alive.

Both are excluded from the cycle graph, so a field marked either way satisfies the compiler's check.

#### Runtime behavior

Neither modifier contributes to its referent's strong reference count, so declaring one breaks the underlying ARC cycle for real, not just at the type-check level:

- **`weak`** fields are automatically reset to `Option.None` the instant their referent's last strong reference is released — you never observe a dangling pointer, only `None` a little earlier than you might expect:

    ```dream
    class Node {
        public value: int;
        public weak parent: Option<Node>;
    }

    fun demo(child: Node) {
        let p = Node(...);
        child.parent = Option.Some(p);
        // ... p's only strong owner is this local ...
    } // `p` is released here -> `child.parent` becomes `Option.None`
    ```

- **`unowned`** fields hold the referent's raw, unretained pointer. Reading one after its referent has been freed **traps** (a fatal runtime panic), rather than reading freed memory — `unowned` is a promise ("this will always outlive me") the runtime checks for you at the point of failure, even though it can't prevent the failure itself:

    ```
    panic: access to deallocated 'unowned' reference (at cache.dream:12, in main)
    ```

    Use `unowned` only when you can truly guarantee the referent outlives every access; reach for `weak` (and a `switch`/`is_some()` check) whenever the referent's lifetime is less certain.

When the referent is freed, every live `weak` slot watching it becomes `None`, and every `unowned` slot is poisoned so later reads panic.

### `@allow_cycle`: the escape hatch

For the rare case where a cycle is intentional and manually managed, annotate **every** class in the cycle:

```dream
@allow_cycle
class Node {
    public next: Node;
    public prev: Node;   // author takes manual responsibility for breaking this cycle
}
```

`@allow_cycle` only suppresses a cycle that is entirely contained within the classes carrying it — annotating just one class in a multi-class cycle does not launder the rest of it.

## `@unsafe`: manual memory management

ARC covers every allocation by default, but a handful of low-level primitives step outside it for tight, allocation-sensitive hot paths: `Buffer.realloc`/`Buffer.free` and [`Pointer<T>`](arrays.md#pointert-manual-allocation-unsafe) manage a block's lifetime directly through the allocator, bypassing reference counting entirely. Every function or method that touches one of these must be marked `@unsafe`:

```dream
@unsafe
fun grow(p: Pointer<int>): Pointer<int> {
    p.realloc(p.len() * 2);   // fine: this function is itself @unsafe
    return p;
}

fun caller(): void {
    let p = Pointer<int>.alloc(4);
    grow(p);   // error: call to '@unsafe' function 'grow' is only allowed from
               // another '@unsafe' function or method
}
```

`@unsafe` is purely a caller-side gate, checked at every call site (not just where the callee is declared): calling an `@unsafe` function/method from ordinary code is a compile-time error, exactly like calling an `unsafe fn` from safe code in Rust. Marking your own function `@unsafe` propagates the same restriction to *its* callers — the attribute has to be threaded all the way up to wherever the unsafe operation is actually justified.

What `@unsafe` does **not** do: it does not insert runtime checks, and it does not verify the specific contract of the operation you're calling (e.g. that a `Buffer.realloc`'d array has exactly one owner, or that a freed `Pointer<T>` is never read again). It is a documented promise from the author, not a proof — the same trade-off manual memory management makes in every language that offers it.
