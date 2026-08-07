# Collections

**Package:** `system.collections` — `import system.collections;`

Growable collections: `List<T>`, `Map<K, V>`, `Set<T>`, `Queue<T>`, and `Stack<T>`. All support `for..in` and share `.length`. Bootstrap [`Collection<T>`](../language/arrays.md#collection-protocols) predicates need no import; query helpers (`filter`, `map`, …) need this package.

```dream
import system;
import system.collections;
```

## Literal syntax

When the target type is unambiguous (`let` annotation, parameter, field, …):

```dream
let nums: List<int> = [1, 2, 3];
let users: Set<string> = {"alice", "bob"};
let scores: Map<string, int> = {"alice": 95, "bob": 80};
```

- `[e1, e2, ...]` → `List<T>` only when the expected type is `List<T>` (otherwise a plain array).
- `{e1, e2, ...}` → `Set<T>` (duplicates dropped).
- `{k1: v1, ...}` → `Map<K, V>` (`:` after the first element distinguishes Map from Set).
- Empty `[]` / `{}` need a typed target: `let xs: Set<int> = {};`.

## Query helpers (`Collection<T>`)

### Bootstrap (no import)

#### `all(pred: fun(T): bool): bool`

Returns `true` when every element satisfies the predicate. Use it for validation rules that must hold for the whole collection (e.g. "all scores are non-negative").

```dream
let xs: List<int> = [2, 4, 6];
System.println(xs.all((n: int): bool => n % 2 == 0));  // true
```

#### `any(pred: fun(T): bool): bool`

Returns `true` when at least one element satisfies the predicate. Prefer this over `!none(pred)` when you only need to know whether a match exists.

```dream
System.println(xs.any((n: int): bool => n > 5));  // true
```

#### `none(pred: fun(T): bool): bool`

Returns `true` when no element satisfies the predicate. Equivalent to `!any(pred)` but reads clearly for "must not contain" checks.

```dream
System.println(xs.none((n: int): bool => n < 0));  // true
```

#### `count_where(pred: fun(T): bool): int`

Counts how many elements satisfy the predicate. Use when you need a tally rather than the elements themselves.

```dream
System.println(xs.count_where((n: int): bool => n > 3));  // 2
```

#### `find_where(pred: fun(T): bool): Option<T>`

Returns the first matching element, or `None`. Prefer over `filter` when you only need one hit and want to stop early.

```dream
System.println(xs.find_where((n: int): bool => n > 3).unwrap_or(0));  // 4
```

#### `for_each(action: fun(T): void): void`

Runs a side-effect callback on every element. Use for logging or mutation; prefer `map` when you need a transformed collection.

```dream
xs.for_each((n: int): void => { System.println(n); });
```

#### `is_empty(): bool` / `.length`

Reports whether the collection has no elements and how many it holds. `.length` is the element count shared by all collection types.

```dream
System.println(xs.is_empty());  // false
System.println(xs.length);      // 3
```

#### `first(): Option<T>` / `last(): Option<T>` (`IndexedCollection`)

Returns the first or last element without removing it. Empty collections yield `None` — safer than indexing when length may be zero.

```dream
System.println(xs.first().unwrap_or(0));  // 2
System.println(xs.last().unwrap_or(0));   // 6
```

### Package extensions (`import system.collections;`)

#### `to_list(): List<T>`

Materializes the collection into a new `List<T>` snapshot. Use when you need indexed access or list-specific APIs after working with another collection type.

```dream
let copy = xs.to_list();
```

#### `filter(pred: fun(T): bool): List<T>`

Returns a new list of elements that pass the predicate. Prefer over manual loops when building a subset to chain with other query helpers.

```dream
let evens = xs.filter((n: int): bool => n % 2 == 0);
```

#### `map<U>(f: fun(T): U): List<U>`

Transforms each element into a new value, producing a new list. The workhorse for projection — pair with `filter` or `reduce` for pipelines.

```dream
let doubled = xs.map((n: int): int => n * 2);  // [4, 8, 12]
```

#### `flat_map<U>(f: fun(T): List<U>): List<U>`

Maps each element to a list, then concatenates the results. Use when each input may produce zero or many outputs (e.g. splitting lines into words).

```dream
let flat = xs.flat_map((n: int): List<int> => List.from_array([n, n + 1]));
```

#### `reduce<A>(init: A, f: fun(A, T): A): A`

Folds the collection into a single accumulator value. Prefer over a manual loop when the operation is associative and you want a clear seed.

```dream
let sum = xs.reduce(0, (acc: int, n: int): int => acc + n);  // 12
```

#### `collect_set(): Set<T>`

Builds a `Set<T>` from the collection, dropping duplicates. Use after `map` when uniqueness matters more than order.

```dream
let unique = List.from_array([1, 1, 2]).collect_set();
```

#### `distinct(): List<T>`

Returns a new list with duplicates removed, preserving first-seen order. Prefer over `collect_set` when you need a list result with stable ordering.

```dream
let d = List.from_array([1, 2, 1]).distinct();  // [1, 2]
```

#### `take(n: int): List<T>` / `skip(n: int): List<T>`

Returns the first `n` elements or drops the first `n`. Combine them for pagination-style slicing without mutating the source.

```dream
let head = xs.take(2);   // [2, 4]
let rest = xs.skip(1);   // [4, 6]
```

#### `order_by(cmp: fun(T, T): int): List<T>`

Returns a new list sorted by the comparator (`< 0`, `0`, `> 0`). Use for custom sort keys; prefer `List.sort()` when `T` is `Comparable<T>` and you can sort in place.

```dream
let sorted = xs.order_by((a: int, b: int): int => a.compare(b));
```

#### `seq(): Seq<T>`

Wraps the collection in a lazy chaining facade over an eager `List` snapshot. Prefer `seq()` when chaining several transforms before materializing with `to_list()`.

```dream
let out = xs.seq()
    .filter((n: int): bool => n > 2)
    .map((n: int): int => n * 10)
    .to_list();  // [40, 60]
```

`Seq<T>` methods: `filter`, `map`, `take`, `skip`, `distinct`, `order_by`, `flat_map`, `to_list` — same meaning as above, returning `Seq` until `to_list()`.

## `List<T>`

Growable sequence with O(1) random access and amortized O(1) append. Bracket indexing returns `Option<T>` on read.

```dream
let nums = List<int>();
nums.push(10);
nums.push(20);
nums[1] = 99;
let first = nums[0];  // Option<int>
for (let n in nums) {
    System.println(n);
}
```

### Construction

#### `List(capacity: int = 8)`

Creates an empty list, optionally reserving capacity to reduce reallocations when you know the approximate size.

```dream
let a = List<int>();
let b = List<int>(32);
```

#### `List.from_array(items: T[]): List<T>` (static)

Copies a fixed array into a new growable list. Use when you have array literals or `Buffer` data and need list APIs.

```dream
let nums = List.from_array([1, 2, 3]);
```

### Size

#### `.length` / `is_empty(): bool` / `capacity(): int`

Reports element count, emptiness, and allocated slots (capacity is always ≥ length). Check `capacity()` when tuning performance for large append-heavy workloads.

```dream
System.println(nums.length);     // 3
System.println(nums.is_empty()); // false
System.println(nums.capacity()); // >= 3
```

### Mutating

#### `push(value: T): void`

Appends one element at the end. The primary way to grow a list — amortized O(1).

```dream
nums.push(4);
```

#### `push_all(items: T[]): void`

Appends every element from an array in order. Prefer over repeated `push` when bulk-adding from another array.

```dream
nums.push_all([5, 6]);
```

#### `insert(index: int, value: T): bool`

Inserts at `index` (may equal `length` to append). Returns `false` if out of range — use when you need ordered insertion, not just append.

```dream
nums.insert(0, 0);  // true
```

#### `set(index: int, value: T): bool` / `nums[i] = value`

Writes a value at an index. Bracket assignment is shorthand; both return `false` on out-of-range indices.

```dream
nums.set(0, 100);  // true
nums[1] = 200;
```

#### `remove(value: T): bool`

Removes the first element equal to `value`. Returns `false` if not found — O(n) scan from the front.

```dream
nums.remove(100);  // true if present
```

#### `remove_at(index: int): bool`

Removes the element at `index`, shifting later elements left. Prefer when you know the position (e.g. after `index_of`).

```dream
nums.remove_at(0);
```

#### `pop(): Option<T>`

Removes and returns the last element. Returns `None` on an empty list — use for stack-like access at the tail.

```dream
let last = nums.pop().unwrap_or(0);
```

#### `clear(): void`

Removes all elements but keeps the backing capacity for reuse. Call between logical batches to avoid reallocating.

```dream
nums.clear();
```

### Querying

#### `get(index: int): Option<T>` / `nums[i]`

Reads the element at `index` without removing it. Bracket read is equivalent; both return `None` when out of range.

```dream
System.println(nums.get(0).unwrap_or(0));
System.println(nums[0].unwrap_or(0));
```

#### `contains(value: T): bool`

Returns whether any element equals `value`. O(n) linear scan — use a `Set` when membership checks dominate.

```dream
System.println(nums.contains(2));
```

#### `index_of(value: T): Option<int>`

Returns the index of the first equal element, or `None`. Pair with `remove_at` when you need position-aware removal.

```dream
System.println(nums.index_of(2).unwrap_or(0 - 1));
```

### Sorting and search

#### `sort(): void` where `T : Comparable<T>`

Sorts the list in place ascending by natural order. Mutates the list — call `to_list()` on a copy first if you need the original order.

```dream
let xs = List.from_array([3, 1, 2]);
xs.sort();
```

#### `sort_by(cmp: fun(T, T): int): void`

Sorts in place with a custom comparator. Use for descending order, field-based keys, or types without `Comparable<T>`.

```dream
xs.sort_by((a: int, b: int): int => b.compare(a));  // descending
```

#### `binary_search(value: T): Option<int>` where `T : Comparable<T>`

Returns the index of `value` in O(log n) when the list is sorted ascending. Returns `None` if absent — sort first or results are undefined.

```dream
xs.sort();
System.println(xs.binary_search(2).unwrap_or(0 - 1));
```

## `Map<K, V>`

Hash map with average O(1) lookups. Keys need working `hash_code` and `==` (primitives and strings do; classes use reference equality unless overridden). `for..in` yields `KeyValuePair<K, V>` (`key`, `value`).

```dream
let scores = Map<string, int>();
scores.set("alice", 95);
scores["dave"] = 60;
let val = scores["dave"];  // Option<int>
for (let pair in scores) {
    System.println(pair.key);
    System.println(pair.value);
}
```

### Construction

#### `Map(capacity: int = 8)`

Creates an empty map, optionally reserving bucket capacity. Pre-size when you expect many entries to reduce rehashing.

```dream
let m = Map<string, int>();
```

#### `Map.from_arrays(keys: K[], values: V[]): Map<K, V>` (static)

Builds a map from parallel key and value arrays (same length). Convenient for static lookup tables defined as two arrays.

```dream
let m = Map.from_arrays(["a", "b"], [1, 2]);
```

### Size

#### `.length` / `is_empty(): bool` / `capacity(): int`

Reports entry count, emptiness, and internal bucket capacity. Same semantics as `List` sizing helpers.

```dream
System.println(m.length);
System.println(m.is_empty());
System.println(m.capacity());
```

### Mutating

#### `set(key: K, value: V): void` / `m[key] = value`

Inserts or overwrites an entry. Bracket assignment is sugar — both upsert the key.

```dream
m.set("alice", 95);
m["bob"] = 80;
```

#### `set_all(keys: K[], values: V[]): void`

Bulk-inserts from parallel arrays. Prefer over repeated `set` when loading a batch of pairs.

```dream
m.set_all(["c", "d"], [3, 4]);
```

#### `remove(key: K): bool`

Removes the entry for `key` if present. Returns `false` when the key was absent.

```dream
m.remove("alice");  // true if it existed
```

#### `clear(): void`

Removes all entries while keeping allocated capacity. Use between logical sessions on a reused map.

```dream
m.clear();
```

### Querying

#### `get(key: K): Option<V>` / `m[key]`

Looks up a value by key without removing it. Returns `None` for missing keys — prefer `get_or` when you want a default inline.

```dream
System.println(m.get("bob").unwrap_or(0));
```

#### `get_or(key: K, fallback: V): V`

Returns the stored value or `fallback` when the key is absent. Avoids nested `unwrap_or` at call sites.

```dream
System.println(m.get_or("missing", -1));
```

#### `contains(key: K): bool`

Returns whether the key exists. Use for presence checks without fetching the value.

```dream
System.println(m.contains("bob"));
```

#### `keys(): K[]` / `values(): V[]`

Returns snapshots of all keys or all values (order not guaranteed). Use when iterating one side without pairs.

```dream
let ks = m.keys();
let vs = m.values();
```

### `KeyValuePair<K, V>`

Holds one map entry's key and value. Constructed by `for..in` over a map or manually when building pair lists.

```dream
let pair = KeyValuePair<string, int>("x", 1);
System.println(pair.key);    // x
System.println(pair.value);  // 1
```

## `Set<T>`

Hash set of unique values (same key requirements as `Map`).

```dream
let users = Set<string>();
users.add("alice");
users.add("alice");  // false — already present
```

### Construction

#### `Set(capacity: int = 8)`

Creates an empty set with optional reserved capacity. Use when you expect many unique inserts.

```dream
let s = Set<int>();
```

#### `Set.from_array(items: T[]): Set<T>` (static)

Builds a set from an array, dropping duplicates. Convenient for deduplicating a literal or scan result in one step.

```dream
let s = Set.from_array([1, 2, 2, 3]);  // {1, 2, 3}
```

### Size

#### `.length` / `is_empty(): bool` / `capacity(): int`

Reports unique element count, emptiness, and bucket capacity. Same pattern as `Map` and `List`.

```dream
System.println(s.length);
System.println(s.is_empty());
System.println(s.capacity());
```

### Mutating

#### `add(value: T): bool`

Inserts `value` if not already present. Returns `true` when newly added, `false` if duplicate — useful for idempotent registration.

```dream
s.add(4);
```

#### `add_all(items: T[]): void`

Adds every element from an array, ignoring duplicates. Bulk alternative to repeated `add`.

```dream
s.add_all([5, 6, 5]);
```

#### `remove(value: T): bool`

Removes `value` if present. Returns `false` when the value was not in the set.

```dream
s.remove(4);
```

#### `clear(): void`

Removes all elements while retaining capacity for reuse.

```dream
s.clear();
```

### Querying

#### `contains(value: T): bool`

Returns whether `value` is in the set. O(1) average — prefer over `List.contains` when membership dominates.

```dream
System.println(s.contains(2));
```

#### `to_array(): T[]`

Copies all elements into a new array. Order is undefined — use when an array API is required downstream.

```dream
let arr = s.to_array();
```

## `Queue<T>`

FIFO ring buffer implementing `Collection<T>`. Use for breadth-first traversal, job scheduling, or any first-in-first-out pipeline.

#### `Queue()` / `.length`

Creates an empty queue and reports how many elements are waiting. `.length` is zero until you enqueue.

```dream
let q = Queue<int>();
System.println(q.length);  // 0
```

#### `enqueue(value: T): void`

Adds an element at the back. Pair with `dequeue` for FIFO consumption.

```dream
q.enqueue(1);
q.enqueue(2);
```

#### `dequeue(): Option<T>`

Removes and returns the front element. Returns `None` on an empty queue — the primary FIFO drain operation.

```dream
System.println(q.dequeue().unwrap_or(-1));  // 1
```

#### `peek(): Option<T>`

Returns the front element without removing it. Use to inspect the next item before committing to `dequeue`.

```dream
System.println(q.peek().unwrap_or(-1));  // 2
```

#### `clear(): void`

Empties the queue while keeping the ring buffer allocated.

```dream
q.clear();
```

```dream
for (let n in q) {
    System.println(n);
}
```

## `Stack<T>`

LIFO wrapper over `List<T>`. Use for undo buffers, depth-first traversal, or nested scope unwinding.

#### `Stack()` / `.length`

Creates an empty stack and reports depth. Backed by a `List` — same growth characteristics as `List.push`.

```dream
let st = Stack<int>();
System.println(st.length);  // 0
```

#### `push(value: T): void`

Pushes onto the top. Most recent push is returned first by `pop`.

```dream
st.push(10);
st.push(20);
```

#### `pop(): Option<T>`

Removes and returns the top element. Returns `None` when empty.

```dream
System.println(st.pop().unwrap_or(-1));  // 20
```

#### `peek(): Option<T>`

Returns the top element without removing it. Inspect before `pop` when order matters.

```dream
System.println(st.peek().unwrap_or(-1));  // 10
```

#### `clear(): void`

Removes all elements from the stack.

```dream
st.clear();
```

Iteration is bottom-to-top (same order as the underlying list).
