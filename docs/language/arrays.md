# Arrays

## Creating an array

Write a comma-separated list of values inside `[...]`. All elements must be the same type:

```dream
let nums = [1, 2, 3, 4, 5];       // int[]
let words = ["red", "green", "blue"]; // string[]
```

## Reading and writing elements

Zero-indexed bracket access:

```dream
let first = nums[0];   // 1
nums[2] = 99;          // [1, 2, 99, 4, 5]
```

Going out of bounds is undefined behaviour — there is no automatic bounds check at runtime.

## Array size

Use the `size` method to get the number of elements. It is the same `size()` the stdlib `List` and
`Map` expose, so every collection is measured the same way:

```dream
let count = nums.size();   // 5
```

## Passing arrays to functions

Arrays are reference types. Passing an array to a function does not copy it; both the caller and the callee see the same backing buffer:

```dream
fun fill_zeros(arr: int[]): void {
    let i = 0;
    while (i < arr.size()) {
        arr[i] = 0;
        i = i + 1;
    }
}
```

## Fixed size

Arrays created from literals (or via the raw `Buffer.alloc<T>(n)` primitive) are fixed-size — a
plain `T[]` backing buffer. You cannot push or pop from them.

For a low-level, zero-initialized buffer of a runtime length, use `Buffer.alloc`:

```dream
let buf = Buffer.alloc<int>(4);   // int[] of length 4, all zero
buf[0] = 10;
```

## Growable arrays: `Array<T>`

`Array<T>` is the general-purpose growable collection: a class wrapping a `T[]` buffer that doubles
on demand. Construct one with `Array<T>()` and use its methods:

```dream
let xs = Array<int>();
xs.push(10);
xs.push(20);
println(xs.size());          // 2
println(xs.get(0).unwrap_or(-1)); // 10
```

It offers `push`, `pop`, `get`/`set` (also the `xs[i]` indexer), `contains`, `index_of`,
`remove_at`, `clear`, `iterator` (so `for (let x in xs)` works), and `sort_by`. When the element
type is `Comparable`, `sort()` and `binary_search()` are also available:

```dream
let ys = Array<int>();
ys.push(3);
ys.push(1);
ys.push(2);
ys.sort();                       // 1, 2, 3
println(ys.binary_search(2).unwrap_or(-1)); // 1
```

[`List<T>`](../stdlib/collections.md) is a near-identical growable collection; both `Array<T>` and
`List<T>` implement the shared [`Collection<T>`](#the-collection-interface) interface.

## The `Collection<T>` interface

`Array<T>` and `List<T>` both implement `Collection<T>`, which exposes `size()` and `get(index)`
plus the default methods `is_empty()`, `first()`, and `last()`. A function can accept any collection
by that interface and dispatch dynamically:

```dream
fun sum(xs: Collection<int>): int {
    let total = 0;
    let i = 0;
    while (i < xs.size()) {
        total = total + xs.get(i).unwrap_or(0);
        i = i + 1;
    }
    return total;
}
```

## Array of classes

```dream
class Point { x: int; y: int; }

let pts: Point[] = [
    Point(0, 0),
    Point(1, 2),
];
println(pts[1].x);   // 1
```

## Nested arrays

The element type can itself be an array (or any other type), giving multi-dimensional arrays:

```dream
let grid: int[][] = [[1, 2, 3], [4, 5, 6]];
println(grid.size());      // 2  (rows)
println(grid[0].size());   // 3  (columns)
println(grid[1][2]);      // 6
```
