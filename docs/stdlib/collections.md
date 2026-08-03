# Collections

The standard library ships three growable collection types, available in every program with no import: `List<T>`, `Map<K, V>`, and `Set<T>`. All three support `for..in` iteration and share the common `size()` method.

## Literal syntax

All three types can be built with a literal instead of a constructor call plus individual inserts, whenever the surrounding context (a `let` type annotation, a function parameter/return type, a field type, etc.) makes the target type unambiguous:

```dream
let nums: List<int> = [1, 2, 3];
let users: Set<string> = {"alice", "bob"};
let scores: Map<string, int> = {"alice": 95, "bob": 80};
```

- `[e1, e2, ...]` builds a `List<T>` — but only when the expected type is specifically `List<T>`; with no such context (or an `int[]`-typed context) `[...]` still means a plain array, exactly as before.
- `{e1, e2, ...}` builds a `Set<T>`; duplicates are silently deduplicated, same as calling `.add()` for each.
- `{k1: v1, k2: v2, ...}` builds a `Map<K, V>`; a `:` after the first element is what distinguishes a Map literal from a Set literal.
- An empty literal (`[]`, `{}`) needs the target type spelled out somewhere, since there is no element to infer it from — e.g. `let xs: Set<int> = {};`. An empty `{}` is valid for both `Set<T>` and `Map<K, V>`, disambiguated by the annotation.

Each literal lowers to a single bulk call — `List<T>.from_array(...)`, `Set<T>.from_array(...)`, or `Map<K, V>.from_arrays(...)` — not one `.push`/`.add`/`.set` call per element, so a literal with N elements costs one call, not N. These factories (along with the bulk `push_all`/`add_all`/`set_all` instance methods they're built on) are also callable directly.

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
- `.push_all(items)` — append every element of an array, in order (what the `[...]` literal desugars to).
- `List<T>.from_array(items)` (static) — build a new list from an array; what the `[...]` literal desugars to.
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
scores.set("alice", 95);
scores.set("bob", 80);
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

- `.set(key, value)` — insert or update.
- `.set_all(keys, values)` — insert/update from parallel `keys`/`values` arrays (what the `{k: v, ...}` literal desugars to).
- `Map<K, V>.from_arrays(keys, values)` (static) — build a new map from parallel arrays; what the `{k: v, ...}` literal desugars to.
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
- `.add_all(items)` — insert every element of an array, duplicates ignored (what the `{...}` literal desugars to).
- `Set<T>.from_array(items)` (static) — build a new set from an array; what the `{...}` literal desugars to.
- `.contains(value)` — membership.
- `.remove(value)` — remove, returning `true` if it existed.
- `.size()` / `.clear()` — count and empty.
- `.to_array()` — a new array of all elements.

Sets iterate with `for..in`, and their element requirements match `Map` keys (working `hash_code` and `==`; classes use reference equality by default).
