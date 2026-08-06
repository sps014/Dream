# Comments as documentation

Dream uses ordinary contiguous `//` comments as API documentation. The language has no separate `///` or block-doc syntax — a short sentence above each public type, constructor, and method is the convention.

```dream
// Number of elements currently stored.
public get size(): int {
    return this.count;
}
```

The LSP extracts these for hover: it walks upward from a declaration through contiguous `//` lines (blank lines break the block). Private helpers and non-public `fun`s may omit comments.

Prefer one concise sentence that states behavior and edge cases (`None` when empty, case-sensitivity, …), matching the style of `List` and `int.parse` in the stdlib.
