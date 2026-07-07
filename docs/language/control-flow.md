# Control Flow

Control flow decides which code runs and how often. Dream has the usual `if`, loops, and `switch`, plus labeled loops for the tricky cases.

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

Conditions are parenthesized and must be `bool`. For selecting a *value*, the ternary `cond ? a : b` is often cleaner — see [Operators](operators.md).

## Loops

### while

Runs the body while the condition holds:

```dream
let i = 0;
while (i < 10) {
    println(i);
    i = i + 1;
}
```

### do / while

Same as `while`, but the condition is checked at the end, so the body always runs at least once:

```dream
let i = 0;
do {
    println(i);
    i = i + 1;
} while (i < 3);
```

### for

A three-part loop: initializer, condition, increment. All three parts are optional. The initializer runs once, the condition is checked before each pass, and the increment runs after each body:

```dream
for (let i = 0; i < 5; i = i + 1) {
    println(i);
}
```

### for-each

Iterate a collection's elements directly with `for (let x in ...)`. The loop variable takes each element in turn:

```dream
let xs: int[] = [10, 20, 30];
for (let value in xs) {
    println(value);
}
```

`for..in` also works over a `string` (yielding each `char`) and over any type implementing the enumerator protocol — including `List` and `Map`. See [Indexers and enumerators](classes-structs.md#indexers-and-enumerators).

```dream
for (let c in "abc") {
    println(c);   // 'a', 'b', 'c'
}
```

## break and continue

`break` leaves the nearest loop; `continue` skips to its next iteration:

```dream
for (let i = 0; i < 10; i = i + 1) {
    if (i % 2 == 0) {
        continue;   // skip even numbers
    }
    println(i);
}
```

Using either outside a loop is a compile error.

## switch

`switch` has two forms, and the parser picks based on the body:

- A **C-style** switch (below) starts with `case`/`default` and matches against constant labels.
- A **pattern-matching** switch uses `pattern => body` arms to destructure [discriminated unions](enums-unions.md).

The C-style form has **no fallthrough** — each `case` runs only its own block. A case may list comma-separated labels, and `default` is optional:

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

Labels must be constants (integers, strings, booleans, or enum members) that match the subject's type. Duplicate labels are an error. Enums work naturally:

```dream
enum Color { Red, Green, Blue }

switch (c) {
    case Color.Red:   print("red\n");
    case Color.Green: print("green\n");
    default:          print("other\n");
}
```

## Advanced: labeled loops

Give a loop a label so `break`/`continue` can target an outer loop from inside a nested one:

```dream
outer: for (let i = 0; i < 3; i = i + 1) {
    for (let j = 0; j < 3; j = j + 1) {
        if (j == 1) {
            continue outer;   // next iteration of the outer loop
        }
        if (i == 2) {
            break outer;      // exit both loops
        }
        println(i * 10 + j);
    }
}
```

Targeting a label that is not an enclosing loop is a compile error.
