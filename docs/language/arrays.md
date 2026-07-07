# Arrays

An array is a fixed-size, ordered block of same-typed values. Arrays are reference types, so passing one around shares the same buffer rather than copying it. For a growable sequence, reach for `Array<T>` or [`List<T>`](../stdlib/collections.md).

## Creating, reading, writing

List the values inside `[...]`; all elements must share a type. Access is zero-indexed:

```dream
let nums = [1, 2, 3, 4, 5];            // int[]
let words = ["red", "green", "blue"];  // string[]

let first = nums[0];   // 1
nums[2] = 99;          // [1, 2, 99, 4, 5]
```

!!! warning
    Indexing out of bounds is undefined behavior — there is no runtime bounds check.

## Size

`.size()` returns the element count. It is the same `size()` that `List` and `Map` expose, so every collection is measured the same way:

```dream
let count = nums.size();   // 5
```

## Passing to functions

Because arrays are references, a function sees the caller's buffer directly:

```dream
fun fill_zeros(arr: int[]): void {
    let i = 0;
    while (i < arr.size()) {
        arr[i] = 0;
        i = i + 1;
    }
}
```

## Arrays of classes and nested arrays

The element type can be a class, or another array for multi-dimensional data:

```dream
class Point { x: int; y: int; }

let pts: Point[] = [ Point(0, 0), Point(1, 2) ];
println(pts[1].x);   // 1

let grid: int[][] = [[1, 2, 3], [4, 5, 6]];
println(grid.size());      // 2  (rows)
println(grid[0].size());   // 3  (columns)
println(grid[1][2]);       // 6
```

## Fixed-size buffers

Array literals — and `Buffer.alloc<T>(n)` — produce a fixed-size `T[]`; you cannot push or pop. Use `Buffer.alloc` for a zero-initialized buffer of a runtime length:

```dream
let buf = Buffer.alloc<int>(4);   // int[] of length 4, all zero
buf[0] = 10;
```

## Advanced: growable arrays

### `Array<T>`

`Array<T>` is a class wrapping a `T[]` buffer that doubles on demand:

```dream
let xs = Array<int>();
xs.push(10);
xs.push(20);
println(xs.size());                // 2
println(xs.get(0).unwrap_or(-1));  // 10
```

It offers `push`, `pop`, `get`/`set` (and the `xs[i]` indexer), `contains`, `index_of`, `remove_at`, `clear`, `iterator` (so `for (let x in xs)` works), and `sort_by`. When the element type is `Comparable`, `sort()` and `binary_search()` are also available:

```dream
let ys = Array<int>();
ys.push(3); ys.push(1); ys.push(2);
ys.sort();                                   // 1, 2, 3
println(ys.binary_search(2).unwrap_or(-1));  // 1
```

[`List<T>`](../stdlib/collections.md) is a near-identical growable collection.

### The `Collection<T>` interface

`Array<T>` and `List<T>` both implement `Collection<T>`, which exposes `size()` and `get(index)` plus the defaults `is_empty()`, `first()`, and `last()`. A function can accept any collection by that interface and dispatch dynamically:

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
