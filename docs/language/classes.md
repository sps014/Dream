# Classes

Classes are user-defined types that group related data together.

> A class is a **reference type**: instances live on the heap and are shared by reference. If you
> want a **value type** with copy semantics (stored inline, copied on assignment) but the same set of
> features, use a [`struct`](value-structs.md) instead.

## Defining a class

```dream
class Point {
    x: int;
    y: int;
}
```

Fields are declared as `name: type;` pairs. A field you never assign starts at its zero value (`0`, `0.0`, `false`, or `null`).

## Creating an instance

Create an instance with a constructor call. A class with no explicit `constructor` has an implicit
**zero-argument** default constructor, so you build it with `Point()` and then set its (public)
fields:

```dream
let p = Point();
p.x = 3;
p.y = 4;
```


## Constructors

A class with no `constructor` has an implicit **zero-argument** default constructor (`Point()`);
every field starts at its zero value.

To accept arguments — or run custom logic — when an instance is created, define a `constructor(...)`.
When a `constructor` is present, the constructor call matches its parameters, and any field you do
not assign starts at its zero value (`0`, `0.0`, `false`, or `null`):

```dream
class Account {
    owner: string;
    balance: int;

    constructor(owner: string) {
        this.owner = owner;
        this.balance = 100;   // default starting balance
    }
}

fun main(): void {
    let a = Account("Ada");
    println(a.balance);       // 100
}
```

A `constructor` declares no return type — it always produces an instance of its class. Inside the
body, `this` refers to the new instance. A `constructor` cannot be marked `public`.

## Accessing and mutating fields

Use `.`:

```dream
println(p.x);      // 3
p.x = 10;
println(p.x);      // 10
```

## Methods

Define methods inside the class body using `fun`. Methods automatically receive a `this` parameter that refers to the current instance:

```dream
class Counter {
    count: int;

    fun increment(): void {
        this.count = this.count + 1;
    }

    fun get(): int {
        return this.count;
    }
}

fun main(): void {
    let c = Counter();       // count starts at 0
    c.increment();
    c.increment();
    println(c.get());   // 2
}
```

Methods are called with `instance.method(args)`. The `this` parameter is implicit — you do not pass it yourself.

## Properties (get / set)

A class can expose a computed member that looks like a field but runs code on read and write, using
TypeScript-style `get` and `set` accessors:

```dream
class Temperature {
    celsius: float;

    constructor(celsius: float) { this.celsius = celsius; }

    public get fahrenheit(): float {
        return this.celsius * 9.0f / 5.0f + 32.0f;
    }

    public set fahrenheit(value: float) {
        this.celsius = (value - 32.0f) * 5.0f / 9.0f;
    }
}

fun main(): void {
    let t = Temperature(100.0f);
    println(t.fahrenheit);   // 212  -> calls the getter
    t.fahrenheit = 32.0f;    //      -> calls the setter
    println(t.celsius);      // 0
}
```

- Reading `obj.name` calls the getter; writing `obj.name = v` calls the setter.
- A getter takes no parameters and declares a non-`void` return type. A setter takes exactly one
  parameter (its value); its return value is ignored.
- A property may have a getter, a setter, or both. Accessors obey the usual `public`/private
  visibility rules. They cannot be `async` (a getter read must yield the value directly, not a
  `Future`).
- A `static get` / `static set` accessor is read and written through the type itself rather than an
  instance — `Type.name` calls the static getter and `Type.name = v` calls the static setter:

```dream
class Config {
    public static get version(): int { return 7; }
    public static set level(value: int) { println(value); }
}

fun main(): void {
    println(Config.version);   // 7  -> calls the static getter
    Config.level = 3;          //    -> calls the static setter
}
```

- These are distinct from the bracket [indexer](#indexer-get--set) `get(i)` / `set(i, v)`, which are
  ordinary methods bound to `obj[i]`.

## Destructors

Define `del()` to run cleanup logic when an instance is destroyed. A class is destroyed
when its last reference goes out of scope; `del` runs automatically just before the memory is
released, while the fields are still valid. A destructor takes no parameters and has no return
type, and cannot be marked `public`:

```dream
class File {
    name: string;

    constructor(name: string) {
        this.name = name;
    }

    del() {
        print("closing ");
        println(this.name);
    }
}

fun use_file() {
    let f = File("data.txt");
    // ... work with f ...
}                                  // f goes out of scope here -> "closing data.txt"
```

You never call `del` yourself; the runtime invokes it as part of automatic memory management.

## Nullable classes

Append `?` to a class type to allow `null`:

```dream
class Node {
    value: int;
    next: Node?;

    constructor(value: int, next: Node?) {
        this.value = value;
        this.next = next;
    }
}

let head: Node? = null;
head = Node(1, null);
```

## Recursive classes

A class can hold a nullable reference to itself (non-nullable self-references would have infinite size):

```dream
class Node {
    value: int;
    next: Node?;

    fun has_next(): bool {
        return this.next != null;
    }
}
```

## Visibility

Class members — fields and methods — are **private by default**. A private member may only be
read, written, or called from within the declaring type's own methods (instance or static).
Mark a member `public` to allow access from outside:

```dream
class Account {
    public owner: string;   // readable/writable from anywhere
    balance: int;           // private: only Account's own methods may touch it

    constructor(owner: string) {
        this.owner = owner;
        this.balance = 100;
    }

    public fun deposit(amount: int): void {
        this.balance = this.balance + amount;   // OK: inside Account
    }
}

fun main(): void {
    let a = Account("Ada");
    println(a.owner);        // OK: public field
    a.deposit(50);           // OK: public method
    // println(a.balance);   // error: 'balance' is private to 'Account'
}
```

## Public classes

Mark a class `public` to make it module-visible and expose it to the WebAssembly host:

```dream
public class Vec2 {
    public x: float;
    public y: float;
}
```

## Indexers and enumerators

A class can opt into bracket-indexing (`obj[i]`) and `for..in` iteration by defining
convention-named methods. These are ordinary methods — the special behaviour applies only at the
sugar sites (`obj[i]`, `obj[i] = v`, `for (let x in obj)`); calling `obj.get(i)` directly is always
just a normal method call.

### Indexer: `get` / `set`

- `obj[i]` desugars to `obj.get(i)`. The result type is whatever `get` returns.
- `obj[i] = v` desugars to `obj.set(i, v)` (the return value is discarded).

```dream
class Grid {
    cells: int[];

    constructor(size: int) { this.cells = Array.new<int>(size); }

    public fun get(index: int): int { return this.cells[index]; }
    public fun set(index: int, value: int): void { this.cells[index] = value; }
}

fun main(): void {
    let g = Grid(9);
    g[4] = 42;              // -> g.set(4, 42)
    println(g[4]);          // -> g.get(4)  => 42
}
```

### Enumerator: `iterator` / `next`

`for (let x in obj)` requires `obj.iterator()` to return an enumerator object whose
`next(): Option<T>` yields the next element. `Some(v)` binds `x` to `v`; `None` ends the loop. A
class may `return this;` from `iterator()` to be its own enumerator.

```dream
class RangeIter {
    current: int;
    end: int;

    constructor(start: int, end: int) { this.current = start; this.end = end; }

    public fun next(): Option<int> {
        if (this.current >= this.end) { return Option.None; }
        let v = this.current;
        this.current = this.current + 1;
        return Option.Some(v);
    }
}

class Range {
    start: int;
    end: int;

    constructor(start: int, end: int) { this.start = start; this.end = end; }
    public fun iterator(): RangeIter { return RangeIter(this.start, this.end); }
}

fun main(): void {
    for (let x in Range(0, 5)) { println(x); }   // 0 1 2 3 4
}
```

`break` and `continue` work inside the loop body as usual; because `next()` is re-called at the top
of every iteration, `continue` correctly advances the iterator.

The standard [`List`](../stdlib/list.md) and [`Map`](../stdlib/map.md) already implement both
protocols, so `list[i]`, `list[i] = v`, `map[k]`, `map[k] = v`, and `for..in` over them work out of
the box. Iterating a `Map` yields `KeyValuePair<K, V>` values with public `key` and `value` fields.

### Eligibility rules

The sugar only binds to a method that fits the expected shape; otherwise the method stays an
ordinary method and the sugar site reports a targeted error:

| Hook | Requirements |
| --- | --- |
| read `get` | instance (non-`static`), non-`async`, exactly 1 parameter, non-`void` return |
| write `set` | instance, non-`async`, exactly 2 parameters (return type unconstrained) |
| `iterator` | instance, non-`async`, 0 parameters, returns an enumerator object |
| `next` | instance, non-`async`, 0 parameters, returns `Option<T>` |

For example, a `fun get(index: int): void` is a normal method (not an indexer), so `obj[i]` is a
compile error while `obj.get(i)` keeps working. `static` and `async` variants are likewise never
treated as hooks.

## Implementing interfaces

A class can implement one or more [interfaces](interfaces.md) by listing them after a colon
(`class Cat : Animal { ... }`), committing to provide every method the interface declares. A value
can then be used through the interface type with runtime polymorphism.

## Object protocol overrides

Classes can customize how they are printed and hashed by overriding `to_string` and `hash_code`. See [The object type](objects.md) for details.

## Memory

Each class instance is a heap allocation. The memory is freed automatically when the last reference to it drops — no manual `free` needed. If the class defines a `del()` destructor, it runs just before the memory is released. See [Memory Model](../memory.md) for a full explanation.
