# JSON

The prelude ships a native JSON implementation: a `JsonValue` data model, `JSON.parse` / `JSON.stringify`, and a `@json` attribute that derives converters for your own types. It is pure Dream with no interop, so it runs on every host, including the `wasmtime` test harness.

Most of the time you want `@json` auto-derive. Reach for `JsonValue` when you need to build or inspect arbitrary, untyped JSON.

## Auto-derive with `@json`

Mark a class `@json` and the compiler generates its `to_json` / `from_json`, so it round-trips with no boilerplate. It works for a value [`struct`](../language/classes-structs.md) too:

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

- `JSON.serialize(x): string` — stringify any `@json` value.
- `JSON.deserialize<T>(text): T` — parse and reconstruct a `T`.

Field types may be primitives, `string`, other `@json` classes, arrays of those, and nullable `string?` or nullable `@json` classes. A field whose type is a class/struct/union that is *not* `@json` is a compile error naming the type.

### Custom keys

`@property_name("key")` maps a field to a different JSON key while keeping the Dream name in code:

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

A nullable field (`string?` or a nullable `@json` class) maps to JSON `null`. On serialize, a `null` field is written as `null`; on deserialize, a JSON `null` *or* a missing key produces a `null` field:

```dream
@json
class Profile { name: string; nickname: string?; address: Address?; }
```

### Unions

`@json` also works on [discriminated unions](../language/enums-unions.md). A value serializes as an object tagged with a `"type"` key naming the active variant, followed by its payload fields; a unit variant becomes just `{ "type": "<Variant>" }`:

```dream
@json
enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }

let text = JSON.serialize(Shape.Rect(3, 4));   // {"type":"Rect","width":3,"height":4}
let back = JSON.deserialize<Shape>(text);       // Shape.Rect(3, 4)
println(JSON.serialize(Shape.Empty));           // {"type":"Empty"}
```

On deserialize, an unrecognized `"type"` falls back to the first variant. `@json` also works on **generic** classes and unions: each instantiation (e.g. `Box<Point>`) derives its own converters.

!!! note "v1 limits"
    Field and payload types are limited to primitives, `string`, other `@json` classes/unions, type parameters of a generic `@json` type, and (for classes) arrays of those plus nullable `string?` / nullable `@json` classes. Nullable arrays, and arrays in union payloads, are not supported. Calling `serialize`/`deserialize` on a type without a derived converter is a compile-time error.

## The `JsonValue` model

For untyped JSON, `JsonValue` holds any JSON value. Build with the static constructors, read with the typed accessors:

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
| `key_at(index): Option<string>` | object key at insertion index |
| `set(key, v)` / `push(v)` | mutate an object / array |
| `size(): int` | array length |
| `is_null(): bool` | true for `null` |

`get`, `at`, and `key_at` return an `Option` rather than a sentinel, so a miss is explicit. Read with `unwrap_or(JsonValue.none())` or `switch`.

## `JSON.parse` and `JSON.stringify`

```dream
let text = JSON.stringify(user);     // {"name":"Ada","age":36,"tags":["dev"]}

let v = JSON.parse(text);
let none = JsonValue.none();
println(v.get("name").unwrap_or(none).as_string());  // Ada
println(v.get("age").unwrap_or(none).as_int());      // 36
```

`JSON.parse` is a recursive-descent parser. A JSON `null` reads back as a `JsonValue` whose `is_null()` is `true`; a missing object key yields `None` from `get`, so a miss is distinguishable from a present `null`.

`JSON.stringify_pretty(value, indent)` formats with newlines and `indent` spaces per level; an `indent` of `0` matches compact `JSON.stringify`:

```dream
println(JSON.stringify_pretty(v, 2));
// {
//   "name": "Ada",
//   "tags": [
//     "dev"
//   ]
// }
```
