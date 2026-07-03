# Collections

The Dream standard library provides three built-in collection types available automatically in every program: `List<T>`, `Map<K, V>`, and `Set<T>`.

## `List<T>`

`List<T>` is a growable sequence of values with O(1) random access and amortized O(1) appending.

```dream
let nums = List<int>();
nums.push(10);
nums.push(20);

println(nums.size());   // 2
```

### Common List Methods
*   **`.push(value)`**: Appends a value to the end.
*   **`.pop()`**: Removes and returns the last element as an `Option<T>`.
*   **`.get(index)`**: Returns the element at `index` as an `Option<T>`.
*   **`.set(index, value)`**: Overwrites the element at `index`, returning `true` on success.
*   **`.contains(value)`**: Returns `true` if the value is present.
*   **`.index_of(value)`**: Returns the index of the first match as an `Option<int>`.
*   **`.remove_at(index)`**: Removes the element at `index`.
*   **`.clear()`**: Empties the list.
*   **`.sort()`**: Sorts the list in place (requires `T` to implement `Comparable<T>`).
*   **`.sort_by(cmp_func)`**: Sorts the list using a custom comparator function.

### List Indexing and Iteration
Lists support bracket indexing and `for..in` loops:

```dream
nums[1] = 99;                 // sets index 1 to 99
let first = nums[0];          // returns Option<int>

for (let n in nums) {
    println(n);
}
```

---

## `Map<K, V>`

`Map<K, V>` is a hash map with average O(1) lookups and insertions.

```dream
let scores = Map<string, int>();
scores.put("alice", 95);
scores.put("bob", 80);
```

### Common Map Methods
*   **`.put(key, value)` / `.set(key, value)`**: Inserts or updates the value for a key.
*   **`.get(key)`**: Returns the value for `key` as an `Option<V>`.
*   **`.get_or(key, fallback)`**: Returns the value or `fallback` if the key is absent.
*   **`.contains(key)`**: Returns `true` if the key exists.
*   **`.remove(key)`**: Removes the key, returning `true` if it existed.
*   **`.size()`**: Returns the number of pairs.
*   **`.clear()`**: Empties the map.
*   **`.keys()` / `.values()`**: Returns a new array of all keys or all values.

### Map Indexing and Iteration
Maps support bracket indexing and `for..in` loops (yielding `KeyValuePair<K, V>` with `key` and `value` fields):

```dream
scores["dave"] = 60;
let val = scores["dave"];     // returns Option<int>

for (let pair in scores) {
    println(pair.key);
    println(pair.value);
}
```

### Key Requirements
Any type can be a map key as long as its `hash_code` and `==` operators work correctly. Primitives and strings work automatically. Classes use reference equality by default unless their `hash_code` and `==` are overridden.

---

## `Set<T>`

`Set<T>` is a hash set that stores unique values with average O(1) lookups and insertions.

```dream
let users = Set<string>();
users.add("alice");
users.add("bob");
users.add("alice");     // returns false, not added again
```

### Common Set Methods
*   **`.add(value)`**: Inserts a value. Returns `true` if it was newly added, or `false` if it was already present.
*   **`.contains(value)`**: Returns `true` if the value exists in the set.
*   **`.remove(value)`**: Removes the value, returning `true` if it existed.
*   **`.size()`**: Returns the number of elements.
*   **`.clear()`**: Empties the set.
*   **`.to_array()`**: Returns a new array containing all elements in the set.

### Set Iteration
Sets support standard `for..in` loops:

```dream
let unique_numbers = Set<int>();
// ... populate set ...

for (let num in unique_numbers) {
    println(num);
}
```

### Element Requirements
Like maps, any type can be added to a set as long as its `hash_code` and `==` operators work correctly. Classes are compared using reference equality by default.
