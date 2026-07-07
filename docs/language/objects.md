# The `object` Type

`object` is a universal container — it can hold any value: an `int`, a `string`, a class, an array, anything. Use it for heterogeneous data and runtime type dispatch.

## Storing and reading a value

Assigning to an `object` variable automatically **boxes** the value. To read it back, cast with the concrete type; a mismatch traps at runtime:

```dream
let o: object = 42;       // boxing an int
let s: object = "hello";  // boxing a string

let n = (int)o;    // 42, if o holds an int
```

## The `is` operator

Check the runtime type before casting:

```dream
fun describe(o: object): void {
    if (o is int) {
        print("int: ");
        println((int)o);
    } else if (o is string) {
        print("string: ");
        println((string)o);
    } else {
        println("something else");
    }
}
```

On a non-`object` variable, `is` is resolved at compile time — a matching branch is always taken, a non-matching one is eliminated as dead code. `is` also works on [interface](interfaces.md)-typed values, checking the concrete class at runtime.

### `is` with binding

`is` can narrow *and* bind in one step — `expr is Type name` introduces `name: Type` inside the branch, so no separate cast is needed:

```dream
fun describe(o: object): void {
    if (o is int n) {
        println(n + 1);   // `n` is an int, unboxed from `o`
    } else if (o is string s) {
        println(s);       // `s` is a string
    }
}
```

The bound name is visible only inside the taken branch, and works for any target type — primitives are unboxed, reference/interface types are aliased. See [Interfaces](interfaces.md#is-with-binding).

## `to_string` and `hash_code`

Every value responds to `to_string()` (returns `string`) and `hash_code()` (returns `int`), including values stored in an `object`:

```dream
let s = (42).to_string();       // "42"
let h = "hello".hash_code();    // some stable integer
```

### Overriding them on a class

A class customizes `to_string` and `hash_code` by declaring them `@override public`:

```dream
class Color {
    r: int;
    g: int;
    b: int;

    @override public fun to_string(): string {
        return "rgb(" + this.r + ", " + this.g + ", " + this.b + ")";
    }

    @override public fun hash_code(): int {
        return this.r * 65536 + this.g * 256 + this.b;
    }
}
```

Requirements: both `@override` and `public` are required; `to_string` returns `string` and `hash_code` returns `int`, and both take no parameters. Once overridden, `print`/`to_string` on a `Color` — even one stored in an `object` — uses your implementation.
