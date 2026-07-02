# Control Flow

## if / else


```dream
if (score >= 90) {
    print("A\n");
} else if (score >= 70) {
    print("B\n");
} else {
    print("F\n");
}
```

A ternary expression is also available for value selection: `cond ? a : b` (see [operators](operators.md)).

## while

Runs the body repeatedly as long as the condition is `true`:

```dream
let i = 0;
while (i < 10) {
    println(i);
    i = i + 1;
}
```

## for

Three-part loop: initializer, condition, increment. All three parts are optional:

```dream
for (let i = 0; i < 5; i = i + 1) {
    println(i);
}
```

The initializer runs once. The condition is checked before each iteration. The increment runs after each body execution.

## do / while

Like `while`, but the body always runs at least once because the condition is checked at the end:

```dream
let i = 0;
do {
    println(i);
    i = i + 1;
} while (i < 3);
```

## for-each

Iterate the elements of an array directly with `for (let x in arr)`:

```dream
let xs: int[] = [10, 20, 30];
for (let value in xs) {
    println(value);
}
```

The loop variable is bound to each element in turn (its type is the array's element type).

`for..in` also works over any class that implements the enumerator protocol (an `iterator()`
method returning an object with `next(): Option<T>`), including the standard `List` and `Map`, and
over a `string` (which yields its characters as `char`). See
[Indexers and enumerators](classes-structs.md#indexers-and-enumerators).

```dream
for (let c in "abc") {
    println(c);   // 'a', 'b', 'c'
}
```

## switch

`switch` has two forms. The **C-style** form (described here) matches a subject against one or more constant labels; the **pattern-matching** form destructures [discriminated unions](enums-unions.md) with `pattern => body` arms. The parser picks the form from the body: a leading `case`/`default` is the C-style form, anything else is the pattern form.

The C-style form has **no implicit fallthrough** - each `case` runs only its own block. A case may list several comma-separated labels, and a `default` clause is optional:

```dream
switch (code) {
    case 1, 2:
        print("low\n");
    case 3:
        print("three\n");
    default:
        print("other\n");
}
```

Labels must be constants (integers, strings, booleans, or enum members) and match the subject's type. Duplicate labels are a compile error.

## switch over enums

`switch` works naturally with [enums](types.md#enums):

```dream
enum Color { Red, Green, Blue }

switch (c) {
    case Color.Red:
        print("red\n");
    case Color.Green:
        print("green\n");
    default:
        print("other\n");
}
```

## break and continue

`break` exits the nearest enclosing loop immediately:

```dream
let i = 0;
while (true) {
    if (i >= 5) {
        break;
    }
    println(i);
    i = i + 1;
}
```

`continue` skips the rest of the current iteration and goes back to the condition check:

```dream
for (let i = 0; i < 10; i = i + 1) {
    if (i % 2 == 0) {
        continue;   // skip even numbers
    }
    println(i);
}
```

Both `break` and `continue` produce a compile error if used outside a loop.

## Labeled loops

A loop may be given a label so that `break`/`continue` can target an outer loop from within a nested one:

```dream
outer: for (let i = 0; i < 3; i = i + 1) {
    for (let j = 0; j < 3; j = j + 1) {
        if (j == 1) {
            continue outer;   // jump to the next iteration of the outer loop
        }
        if (i == 2) {
            break outer;      // exit both loops
        }
        println(i * 10 + j);
    }
}
```

Targeting a label that is not an enclosing loop is a compile error.
