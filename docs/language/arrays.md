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

Arrays created from literals are fixed-size. You cannot push or pop from them.

If you need a growable array, use [`List<T>`](../stdlib/collections.md):

```dream
let xs = List<int>();
xs.push(10);
xs.push(20);
println(xs.size());   // 2
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
