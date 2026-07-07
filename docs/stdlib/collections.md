# Collections

The standard library ships three growable collection types, available in every program with no import: `List<T>`, `Map<K, V>`, and `Set<T>`. All three support `for..in` iteration and share the common `size()` method.

## `List<T>`

A growable sequence with O(1) random access and amortized O(1) append:

```dream
let nums = List<int>();
nums.push(10);
nums.push(20);
println(nums.size());   // 2
```

Lists support bracket indexing and `for..in`. Indexed reads return an `Option<T>`:

```dream
nums[1] = 99;             // set index 1
let first = nums[0];      // Option<int>

for (let n in nums) {
    println(n);
}
```

Methods:

- `.push(value)` — append.
- `.pop()` — remove and return the last element as `Option<T>`.
- `.get(index)` — element at `index` as `Option<T>`.
- `.set(index, value)` — overwrite, returning `true` on success.
- `.contains(value)` / `.index_of(value)` — membership and first index (`Option<int>`).
- `.remove_at(index)` — remove at `index`.
- `.clear()` — empty the list.

### Sorting

- `.sort()` — in place; requires `T` to implement [`Comparable<T>`](../language/interfaces.md#built-in-equatable-and-comparable).
- `.sort_by(cmp_func)` — in place, using a custom comparator.

## `Map<K, V>`

A hash map with average O(1) lookups and insertions:

```dream
let scores = Map<string, int>();
scores.put("alice", 95);
scores.put("bob", 80);
```

Maps support bracket indexing and `for..in` (yielding a `KeyValuePair<K, V>` with `key` and `value` fields). Indexed reads return an `Option<V>`:

```dream
scores["dave"] = 60;
let val = scores["dave"];   // Option<int>

for (let pair in scores) {
    println(pair.key);
    println(pair.value);
}
```

Methods:

- `.put(key, value)` / `.set(key, value)` — insert or update.
- `.get(key)` — value as `Option<V>`; `.get_or(key, fallback)` — value or `fallback`.
- `.contains(key)` — key present.
- `.remove(key)` — remove, returning `true` if it existed.
- `.size()` / `.clear()` — count and empty.
- `.keys()` / `.values()` — new arrays of all keys or values.

Any type can be a key as long as its `hash_code` and `==` work correctly. Primitives and strings work automatically; classes use reference equality unless their `hash_code` and `==` are overridden.

## `Set<T>`

A hash set of unique values with average O(1) lookups and insertions:

```dream
let users = Set<string>();
users.add("alice");
users.add("bob");
users.add("alice");   // returns false, not added again
```

Methods:

- `.add(value)` — insert; `true` if newly added, `false` if already present.
- `.contains(value)` — membership.
- `.remove(value)` — remove, returning `true` if it existed.
- `.size()` / `.clear()` — count and empty.
- `.to_array()` — a new array of all elements.

Sets iterate with `for..in`, and their element requirements match `Map` keys (working `hash_code` and `==`; classes use reference equality by default).
