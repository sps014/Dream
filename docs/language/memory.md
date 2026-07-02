# Memory Management

Dream manages heap memory for you using **Automatic Reference Counting (ARC)**.

You don't call `free` and there is no garbage collector to pause your program. Memory is reclaimed deterministically the moment the last reference to an object drops.

## What lives on the heap?
- Strings
- Arrays (`T[]`)
- Class instances
- Standard library collections (`List`, `Map`)

Primitive values (`int`, `float`, `bool`, etc.) and value `struct`s are stored directly on the stack or inline within other objects — no heap allocation needed.

## How it works

Every heap-allocated object keeps track of how many references point to it. The compiler inserts `retain` and `release` instructions automatically behind the scenes:

- When a variable **goes out of scope**, its reference is released.
- Reassigning a variable releases the value it previously held.
- When an object's reference count reaches zero, it is immediately freed (and its `del` destructor runs first, if it has one).

You don't have to write any of this yourself. It just works.

```dream
fun make_list(): int[] {
    let arr = [1, 2, 3];   // allocated, count = 1
    return arr;            // handed to caller
}

fun main() {
    let result = make_list(); 
    println(result[0]);
} // result goes out of scope -> count 0 -> freed instantly
```

## Watch out for reference cycles

Because ARC relies on reference counts, it cannot collect reference cycles. 

If class `A` holds a reference to `B`, and `B` holds a reference back to `A`, neither will ever reach a count of zero. This is a memory leak.

```dream
class Node {
    next: Node?;
}
let a = Node();
let b = Node();
a.next = b;
b.next = a; // cycle created!
```

**How to fix cycles:**
Break the cycle with a nullable field (`Node?`) that you manually set to `null` before the objects go out of use, or use a "parent owns children" pattern where children do not hold strong back-references to their parents.