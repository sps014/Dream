# JSON

The prelude provides a native JSON implementation: a `JsonValue` data model, `JSON.parse` /
`JSON.stringify`, and a `@json` attribute that derives converters for your own classes. It is pure
Dream with no interop, so it runs on every host, including the `wasmtime` test harness.

## The `JsonValue` model

A `JsonValue` holds any JSON value. Build values with the static constructors and read them with
the typed accessors:

```dream
let user = JsonValue.dict();
user.set("name", JsonValue.from_string("Ada"));
user.set("age", JsonValue.from_int(36));

let tags = JsonValue.array();
tags.push(JsonValue.from_string("dev"));
user.set("tags", tags);
```

| Constructor | Builds |
| --- | --- |
| `JsonValue.none()` | `null` |
| `JsonValue.boolean(b)` | a boolean |
| `JsonValue.number(d)` / `JsonValue.from_int(n)` | a number |
| `JsonValue.from_string(s)` | a string |
| `JsonValue.array()` | an empty array |
| `JsonValue.dict()` | an empty object |

| Accessor | Returns |
| --- | --- |
| `as_bool()` / `as_int()` / `as_double()` / `as_string()` | the scalar value |
| `get(key): Option<JsonValue>` | object member by key (`None` if absent) |
| `at(index): Option<JsonValue>` | array element by index (`None` if out of range) |
| `key_at(index): Option<string>` | object key at insertion index (`None` if out of range) |
| `set(key, v)` / `push(v)` | mutate an object / array |
| `size(): int` | array length |
| `is_null(): bool` | true for `null` |

`get`, `at`, and `key_at` return an `Option` rather than a sentinel, so a miss is explicit. Read
the value with `unwrap_or(JsonValue.none())` (or `switch`); chaining looks like
`v.get("a").unwrap_or(JsonValue.none()).at(0).unwrap_or(JsonValue.none())`.

## `JSON.parse` and `JSON.stringify`

```dream
let text = JSON.stringify(user);     // {"name":"Ada","age":36,"tags":["dev"]}

let v = JSON.parse(text);
let none = JsonValue.none();
println(v.get("name").unwrap_or(none).as_string());  // Ada
println(v.get("age").unwrap_or(none).as_int());      // 36
```

`JSON.parse` is a recursive-descent parser. A JSON `null` reads back as a `JsonValue` whose
`is_null()` is `true`; a missing object key yields `None` from `get` (so a miss is distinguishable
from a present `null`).

### Pretty-printing

`JSON.stringify_pretty(value, indent)` formats the output with newlines and `indent` spaces per
nesting level. An `indent` of `0` produces compact output, the same as `JSON.stringify`.

```dream
println(JSON.stringify_pretty(v, 2));
// {
//   "name": "Ada",
//   "tags": [
//     "dev"
//   ]
// }
```

## Auto-derive with `@json`

Mark a class `@json` and the compiler generates its `to_json` / `from_json` converters, so the
class round-trips with no boilerplate. This works for a value [`struct`](../language/classes-structs.md)
too — `@json struct Point { public x: int; public y: int; constructor(x: int, y: int) { ... } }`
serializes and deserializes exactly like a class:

```dream
@json
class Address { city: string; zip: string; }

@json
class User { name: string; age: int; address: Address; tags: string[]; }

fun main(): void {
    let u = User("Ada", 36, Address("London", "NW1"), ["dev", "math"]);

    let text = JSON.serialize(u);              // to_json + stringify
    let back = JSON.deserialize<User>(text);   // parse + from_json
    println(back.address.city);                // London
}
```

- `JSON.serialize(x): string` stringifies any `@json` value.
- `JSON.deserialize<T>(text): T` parses `text` and reconstructs a `T`.

Field types may be primitives, `string`, other `@json` classes, arrays of those, and nullable
`string?` or nullable `@json` classes. A field whose type is a class, struct, or union that is *not*
itself `@json` is a compile error — the diagnostic names the type and tells you to add `@json` to it.

### Custom keys

Use `@property_name("key")` on a field to map it to a different JSON key while keeping the Dream
field name in code:

```dream
@json
class Product {
    @property_name("id")
    product_id: int;

    @property_name("priceUsd")
    price: float;
}
```

This writes `product_id` as `"id"` and `price` as `"priceUsd"`.

### Nullable fields

A nullable field (`string?` or a nullable `@json` class) maps to JSON `null`. On serialize, a
`null` field is written as `null`. On deserialize, a JSON `null` or a missing key produces a `null`
field:

```dream
@json
class Profile { name: string; nickname: string?; address: Address?; }

// nickname == null  ->  "nickname":null
// "nickname":null   ->  nickname == null
// a missing key     ->  the field is null too
```

### Unions

`@json` also works on [discriminated unions](../language/enums-unions.md). A value is
serialized as an object tagged with a `"type"` key naming the active variant, followed by that
variant's payload fields; a unit variant becomes just `{ "type": "<Variant>" }`:

```dream
@json
enum Shape {
    Circle(radius: int),
    Rect(width: int, height: int),
    Empty,
}

fun main(): void {
    let text = JSON.serialize(Shape.Rect(3, 4));   // {"type":"Rect","width":3,"height":4}
    let back = JSON.deserialize<Shape>(text);       // Shape.Rect(3, 4)

    println(JSON.serialize(Shape.Empty));           // {"type":"Empty"}
}
```

Payload field types follow the same rules as class fields (primitives, `string`, and other `@json`
classes/unions). On deserialize, an unrecognized `"type"` falls back to the first variant.

`@json` also works on **generic** classes and unions: each instantiation (e.g. `Box<Point>`) derives
its own converters, with generic-typed fields routed through `JSON.serialize` / `JSON.deserialize` for
the concrete type argument.

!!! note "v1 limits"
    Class field and union payload types are limited to primitives, `string`, other `@json`
    classes/unions, type parameters of a generic `@json` type, and (for classes) arrays of those and
    nullable `string?` / nullable `@json` classes (nullable arrays, and arrays in union payloads,
    are not supported). Calling `JSON.serialize` or `JSON.deserialize` on a type without a derived
    converter is a compile-time error.
