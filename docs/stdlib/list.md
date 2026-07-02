# `List<T>`

`List<T>` is part of the standard library and is available in every program — no import needed. It is a growable sequence of values of type `T` with O(1) random access and amortized O(1) `push`.

## Creating a list

```dream
let nums = List<int>();
let words = List<string>();
```

## Methods

### push

Appends a value to the end, growing the backing buffer if needed.

```dream
nums.push(10);
nums.push(20);
nums.push(30);
```

### size

Number of elements currently in the list.

```dream
println(nums.size());   // 3
```

### get

Returns the element at `index` as an `Option<T>`: `Some(value)` when in range, or `None` when `index` is negative or `>= size()`. Use `unwrap_or` (or `switch`) to read it.

```dream
println(nums.get(0).unwrap_or(0 - 1));   // 10
println(nums.get(99).unwrap_or(0 - 1));  // -1 (out of range)
```

### set

Overwrites the element at `index`, returning `true` on success or `false` if `index` is out of range (nothing is written in that case).

```dream
nums.set(1, 99);                         // true
println(nums.get(1).unwrap_or(0 - 1));   // 99
```

### pop

Removes and returns the last element as an `Option<T>`: `Some(value)`, or `None` when the list is empty.

```dream
let last = nums.pop().unwrap_or(0);
```

### contains

Returns `true` if the value is present. Uses value equality (string contents, not pointers).

```dream
println(nums.contains(99));    // true
println(nums.contains(1000));  // false
```

### index_of

Returns the index of the first matching element as an `Option<int>`: `Some(index)`, or `None` if not found.

```dream
let i = nums.index_of(99).unwrap_or(0 - 1);   // 1 (or -1 if absent)
```

### clear

Resets the element count to zero.

```dream
nums.clear();
println(nums.size());   // 0
```

### remove_at

Removes the element at `index`, shifting everything after it left. Returns `true` on success, or `false` if `index` is out of range (the list is left unchanged).

```dream
nums.remove_at(0);   // removes the first element; returns true
```

### iterator

Returns an enumerator so a list can be used directly in a `for..in` loop. You rarely call this
method by hand — `for (let x in list)` calls it for you and binds `x` to each element in order.

```dream
for (let x in nums) {
    println(x);
}
```

## Sorting

A list sorts in place, ascending, with a **stable, O(n log n) merge sort**. There are two forms.

### sort

`sort()` orders the list using each element's [`Comparable<T>`](../language/interfaces.md#built-in-equatable-and-comparable)
`compare` method. **Every primitive** (`int`, `long`, `uint`, `ulong`, `byte`, `char`, `float`,
`double`, `string`) ships a `Comparable` implementation, so `List<int>().sort()`,
`List<string>().sort()`, etc. work out of the box:

```dream
let nums = List<int>();
nums.push(5);
nums.push(1);
nums.push(3);
nums.sort();             // 1, 3, 5

let names = List<string>();
names.push("pear");
names.push("apple");
names.sort();            // apple, pear  (lexicographic)
```

`sort()` is a [constrained extension](../language/generics.md#generic-constraints) (`T : Comparable<T>`),
so a user type sorts once it implements `Comparable` — including a value [`struct`](../language/value-structs.md),
whose `compare` is dispatched statically with no boxing. Calling `sort()` on a type that is *not*
`Comparable` is a compile error.

```dream
class Money : Comparable<Money> {
    public cents: int;
    constructor(cents: int) { this.cents = cents; }
    public fun compare(other: Money): int { return this.cents - other.cents; }
}

let prices = List<Money>();
prices.push(Money(300));
prices.push(Money(100));
prices.sort();           // ascending by cents: 100, 300
```

### sort_by

`sort_by(cmp)` takes a comparator function `fun(T, T): int` that returns a negative number, zero, or a
positive number when the first argument is ordered before, equal to, or after the second. Use it for
custom orderings or element types that are not `Comparable`.

```dream
fun by_desc(a: int, b: int): int { return b - a; }

nums.sort_by(by_desc);   // largest first
```

### binary_search

`binary_search(value)` looks for `value` in a list that is **already sorted ascending** (by `compare`),
returning `Some(index)` of a match or `None` if absent, in O(log n). Like `sort()`, it requires
`T : Comparable<T>`.

```dream
nums.sort();
let at = nums.binary_search(3);   // Some(index) if present, else None
println(at.unwrap_or(0 - 1));
```

## Indexing and iteration

`List` supports the class [indexer and enumerator conventions](../language/classes.md#indexers-and-enumerators).
Because `get` returns `Option<T>`, `list[i]` yields an `Option<T>`, while `for..in` binds the
loop variable to the unwrapped element:

```dream
nums[1] = 99;                      // -> nums.set(1, 99)
let first = nums[0];               // -> nums.get(0)  => Option<int>
for (let x in nums) { /* x: int */ }
```

## Example

```dream
fun main() {
    let xs = List<int>();
    let i = 0;
    while (i < 5) {
        xs.push(i * i);
        i = i + 1;
    }
    // [0, 1, 4, 9, 16]
    println(xs.size());                  // 5
    println(xs.get(4).unwrap_or(0 - 1)); // 16
    println(xs.contains(9));             // true
    xs.remove_at(2);                     // [0, 1, 9, 16]
    println(xs.size());                  // 4
}
```

A `List<T>` grows automatically from a small initial capacity; you never resize it manually.
