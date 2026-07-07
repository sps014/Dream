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
    next: Node?;
}

let a = Node();
let b = Node();
a.next = b;
b.next = a;   // cycle created
```

To avoid this, break the cycle before the objects fall out of use — set a nullable field (`Node?`) back to `null`, or use a "parent owns children" design where children hold no strong back-reference to their parent.
