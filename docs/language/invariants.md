# Language Invariants

Some rules in Dream are permanent by design, not features waiting to be built. The compiler enforces them at parse or analysis time. They are collected here so the resulting errors are never a surprise — code that relies on breaking them is simply not valid Dream.

## Reserved identifiers

Primitive and literal names cannot be reused for a variable, function, parameter, or global:

- Primitives and aliases: `int`, `float`, `double`, `string`, `bool`, `char`, `object`, `void`, `long`, `uint`, `ulong`, `byte`, plus the C#-style spellings `String`, `Int32`, `Int64`, `UInt32`, `UInt64`, `Byte`, `Single`, `Double`, `Boolean`, `Char`, `Object`, `Void`.
- Literals: `true`, `false`, `null`.

The print combinators `__print` / `__println` and any `$`-prefixed name are reserved for the compiler.

```dream
let int = 3;   // error: 'int' is a reserved word
```

## Constructors and destructors

- The constructor is named `constructor`; the destructor is `del`.
- Neither may be `public`, and neither may declare a return type.
- `del` takes no parameters.

Their calling convention is fixed, so their shape is fixed.

## The object protocol

- `@override` applies only to the protocol methods `to_string` and `hash_code`. It must be `public`, take no parameters, and use the fixed return type.
- Any method that overrides a protocol method must be marked `@override`.

## Linkage modifiers are exclusive

`public` and `static` express opposite linkage and cannot combine:

- `public` exposes a symbol to other modules (and, for functions, exports it from the WebAssembly module).
- `static` on a top-level variable pins it to module-internal linkage.

For the same reason, a function cannot be both `public` and `extern` — an `extern` is an imported host symbol, not an exported one.

```dream
public static let x = 1;   // error: cannot be both 'public' and 'static'
```

## Overloading

- Overloads must differ in their parameters; two with identical parameter types are rejected as duplicates.
- Overloads may use default values. An exact-arity match wins over one that fills defaults, and a genuinely ambiguous call is reported at the call site.

## The entry point

`main` cannot be overloaded and must be declared as `main()` or `main(args: string[])`.

## Control flow

- `break` / `continue` are valid only inside a loop, and any label must resolve to an enclosing loop.
- Assigning to a `const` binding is rejected.

## Top-level globals

Globals initialize in declaration order. An initializer may reference earlier globals but not later ones — there are no forward references at module scope.
