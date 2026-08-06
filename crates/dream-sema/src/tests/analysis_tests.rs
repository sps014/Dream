//! Type-checking and diagnostic tests (`test_analyze_*`, switch/union, interfaces, indexers,
//! accessors, is-binding, default params, JS interop). See `harness` for helpers.

use super::harness::*;
use pretty_assertions::assert_eq;

#[test]
fn test_analyze_valid_types() {
    let code = "fun main(): void { let x: int = 5; let y: float = 3.14; let z: string = \"hello\"; let b: bool = true; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_type_mismatch() {
    let code = "fun main(): void { let x: int = \"hello\"; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("cannot convert from int to string")));
}

#[test]
fn test_analyze_new_integer_widening_ok() {
    // The full widening lattice: narrower numeric values flow into wider numeric targets without
    // an explicit cast.
    let code = "fun main(): void {
        let l: long = 5;
        let l2: long = 7u;
        let ul: ulong = 9u;
        let d: double = 9000000000L;
        let i: int = 200b;
        let f: float = 3000000000u;
    }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_new_integer_narrowing_requires_cast() {
    // Assigning a `long` to an `int` is a narrowing conversion and must be rejected without a cast.
    let code = "fun main(): void { let x: int = 5L; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_analyze_new_integer_explicit_casts_ok() {
    // Explicit casts permit narrowing and same-width sign changes between the numeric types.
    let code = "fun main(): void {
        let a: int = (int)9000000000L;
        let b: byte = (byte)511;
        let c: uint = (uint)5;
        let e: long = (long)4000000000u;
    }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_unary_minus_allows_all_numeric_types() {
    // Regression test: unary +/- used to be rejected for `long`/`uint`/`ulong`/`byte`, even though
    // the MIR backend (const-fold + codegen) already handled them like `int`/`float`/`double`.
    let code = "fun main(): void {
        let a: long = -15L;
        let b: uint = -3u;
        let c: ulong = -7ul;
        let d: byte = -(byte)1;
        let e: int = -42;
        let f: float = -1.5f;
        let g: double = -2.5d;
        let h = +5;
    }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_unary_minus_rejects_non_numeric_types() {
    let code = "fun main(): void { let x = -\"hello\"; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("unary +/- requires a numeric type")));
}

#[test]
fn test_analyze_undefined_variable() {
    let code = "fun main(): void { let x = y + 5; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("variable y does not exist")));
}

#[test]
fn test_analyze_array_operations() {
    let code = "
        fun main(): void { 
            let arr: int[] = [1, 2, 3]; 
            let x: int = arr[0];
            arr[1] = 5;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_invalid_array_operations() {
    let code = "
        fun main(): void { 
            let arr: int[] = [1, 2, 3]; 
            arr[\"hello\"] = 5; // Invalid index type
            let x: int = 5;
            x[0] = 1; // Indexing non-array
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Array index must be of type int")));
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Cannot index into non-array type int")));
}

#[test]
fn test_analyze_async_await_valid() {
    // Calling an async fun yields `Future<T>`; awaiting it (at a statement position) yields `T`.
    let code = "
        async fun delay(): void { }
        async fun work(n: int): int { await delay(); return n * 2; }
        async fun main(): void {
            let h = work(3);
            let v = await h;
            let w = await work(4);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_await_outside_async() {
    let code = "async fun delay(): int { return 1; } fun main(): void { let x = await delay(); }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics.diagnostics.iter().any(|d| d
        .message
        .contains("can only be used inside an 'async' function")));
}

#[test]
fn test_analyze_await_in_unconditional_subexpression_allowed() {
    // `await` in an unconditionally-evaluated sub-expression is hoisted by the async normalization
    // pass, so it type-checks cleanly.
    let code = "
        async fun delay(): void { }
        async fun work(n: int): int { await delay(); return n; }
        async fun main(): void { let x = await work(1) + 1; let y = x; }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_await_in_conditional_position_allowed() {
    // `await` in a conditionally-evaluated position (ternary arm) is now supported: the coroutine
    // transform lowers the whole body to a CFG state machine, so it type-checks cleanly.
    let code = "
        async fun delay(): void { }
        async fun work(n: int): int { await delay(); return n; }
        async fun main(): void { let c = true; let x = c ? await work(1) : await work(2); let y = x; }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_await_in_loop_and_branch_allowed() {
    // `await` inside a loop body and a branch body is supported by the CFG coroutine transform.
    let code = "
        async fun step(n: int): int { return n; }
        async fun main(): void {
            let sum = 0;
            let i = 0;
            while (i < 3) { sum = sum + await step(i); i = i + 1; }
            if (sum > 0) { let last = await step(sum); sum = last; }
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_await_non_future_rejected() {
    let code = "async fun main(): void { let x = await 5; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_unresolved_identifier_does_not_cascade() {
    // A single unresolved identifier should report exactly one error: the poison/`Unknown` type it
    // produces unifies with everything, so the downstream `+`, the `: int` annotation, the call
    // argument, and the array index must NOT each add their own follow-on diagnostic.
    let code = "
        fun takes_int(n: int): int { return n; }
        fun main(): void {
            let a: int = missing + 1;
            let b: int = takes_int(missing);
            let arr: int[] = [1, 2, 3];
            let c: int = arr[missing];
        }
    ";
    let diagnostics = analyze_code(code);
    let errors: Vec<&str> = diagnostics
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    // Three uses of `missing`, so three "does not exist" errors -- and nothing else.
    let undefined = errors
        .iter()
        .filter(|m| m.contains("missing does not exist"))
        .count();
    assert_eq!(
        undefined, 3,
        "expected 3 undefined-identifier errors, got: {:?}",
        errors
    );
    assert_eq!(
        errors.len(),
        3,
        "poison type should suppress cascading errors; got: {:?}",
        errors
    );
}

#[test]
fn test_unknown_call_result_does_not_cascade() {
    // Calling an unknown function poisons the result; the inferred variable is poison too, so
    // using it must not pile on more errors.
    let code = "
        fun main(): void {
            let x = nope();
            let y: int = x + 1;
            let z: bool = x;
        }
    ";
    let diagnostics = analyze_code(code);
    let errors: Vec<&str> = diagnostics
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "only the unknown-function error should be reported; got: {:?}",
        errors
    );
}

#[test]
fn test_analyze_union_switch_ok() {
    let code = "
        enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }
        fun area(s: Shape): int {
            return switch (s) {
                Circle(r)  => r * r,
                Rect(w, h) => w * h,
                Empty      => 0,
            };
        }
        fun main(): void { let a: int = area(Shape.Circle(2)); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_union_switch_non_exhaustive() {
    let code = "
        enum Shape { Circle(radius: int), Rect(width: int, height: int), Empty }
        fun area(s: Shape): int {
            return switch (s) {
                Circle(r)  => r * r,
                Rect(w, h) => w * h,
            };
        }
        fun main(): void { let a: int = area(Shape.Empty); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Non-exhaustive switch") && d.message.contains("Empty")));
}

#[test]
fn test_analyze_range_pattern_ok() {
    let code = "
        fun grade(score: int): string {
            return switch (score) {
                90..100 => \"A\",
                _ => \"other\",
            };
        }
        fun main(): void { let s: string = grade(95); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_range_pattern_wrong_bound_type() {
    let code = "
        fun grade(score: int): string {
            return switch (score) {
                \"a\"..\"z\" => \"letters\",
                _ => \"other\",
            };
        }
        fun main(): void { let s: string = grade(95); }
    ";
    let diagnostics = analyze_code(code);
    assert!(diagnostics.has_errors());
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Range pattern bound")));
}

#[test]
fn test_analyze_or_pattern_ok() {
    let code = "
        fun is_vowel(c: char): bool {
            return switch (c) {
                'a' | 'e' | 'i' | 'o' | 'u' => true,
                _ => false,
            };
        }
        fun main(): void { let b: bool = is_vowel('e'); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_or_pattern_exhaustiveness_across_unit_variants() {
    let code = "
        enum Shape { Circle(radius: int), Square, Triangle, Empty }
        fun has_straight_edges(s: Shape): bool {
            return switch (s) {
                Square | Triangle => true,
                Circle(_) | Empty => false,
            };
        }
        fun main(): void { let b: bool = has_straight_edges(Shape.Empty); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(
        diagnostics.has_errors(),
        false,
        "{:?}",
        diagnostics.diagnostics
    );
}

#[test]
fn test_analyze_or_pattern_rejects_binding_alternative() {
    let code = "
        enum Shape { Circle(radius: int), Empty }
        fun f(s: Shape): int {
            return switch (s) {
                Circle(r) | Empty => r,
            };
        }
        fun main(): void { let i: int = f(Shape.Empty); }
    ";
    let diagnostics = analyze_code(code);
    assert!(diagnostics.has_errors());
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("or-pattern alternative cannot bind")));
}

#[test]
fn test_analyze_union_variant_arity_mismatch() {
    let code = "
        enum Shape { Circle(radius: int), Empty }
        fun main(): void { let s: Shape = Shape.Circle(1, 2); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("expects 1 argument")));
}

#[test]
fn test_analyze_generic_union_inference() {
    let code = "
        enum Option<T> { Some(value: T), None }
        fun main(): void {
            let o = Option.Some(42);
            let n: Option<int> = Option.None;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_analyze_switch_expression_arm_type_mismatch() {
    let code = "
        enum Shape { Circle(radius: int), Empty }
        fun f(s: Shape): int {
            return switch (s) {
                Circle(r) => r,
                Empty     => \"oops\",
            };
        }
        fun main(): void { let x: int = f(Shape.Empty); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

// -- `expr?` try-propagation --

#[test]
fn test_try_propagation_result_ok() {
    let code = "
        enum Result<T, E> { Ok(value: T), Err(error: E) }
        fun half(n: int): Result<int, string> {
            if (n % 2 != 0) { return Result.Err(\"odd\"); }
            return Result.Ok(n / 2);
        }
        fun quarter(n: int): Result<int, string> {
            let h = half(n)?;
            return Result.Ok(half(h)?);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_try_propagation_option_ok() {
    let code = "
        enum Option<T> { Some(value: T), None }
        fun first(n: int): Option<int> {
            if (n < 0) { return Option.None; }
            return Option.Some(n);
        }
        fun doubled(n: int): Option<int> {
            let v = first(n)?;
            return Option.Some(v * 2);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_try_propagation_requires_result_or_option_operand() {
    let code = "
        enum Result<T, E> { Ok(value: T), Err(error: E) }
        fun f(): Result<int, string> {
            let n = 5;
            let x = n?;
            return Result.Ok(x);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics.diagnostics.iter().any(|d| d
        .message
        .contains("requires a Result<T, E> or Option<T> operand")));
}

#[test]
fn test_try_propagation_requires_matching_return_type() {
    // `?` on a `Result<..>` inside a function that doesn't itself return a `Result<..>` (here
    // `void`) has nowhere to propagate the `Err` to.
    let code = "
        enum Result<T, E> { Ok(value: T), Err(error: E) }
        fun half(n: int): Result<int, string> {
            if (n % 2 != 0) { return Result.Err(\"odd\"); }
            return Result.Ok(n / 2);
        }
        fun main(): void {
            let h = half(4)?;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics.diagnostics.iter().any(|d| d
        .message
        .contains("requires the enclosing function to return a matching")));
}

#[test]
fn test_try_propagation_rejects_mismatched_error_type() {
    // The enclosing function's `Result<_, E>` error type must match the operand's exactly.
    let code = "
        enum Result<T, E> { Ok(value: T), Err(error: E) }
        fun half(n: int): Result<int, string> {
            if (n % 2 != 0) { return Result.Err(\"odd\"); }
            return Result.Ok(n / 2);
        }
        fun f(n: int): Result<int, int> {
            let h = half(n)?;
            return Result.Ok(h);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_try_propagation_rejects_option_result_mismatch() {
    let code = "
        enum Option<T> { Some(value: T), None }
        enum Result<T, E> { Ok(value: T), Err(error: E) }
        fun first(n: int): Option<int> {
            if (n < 0) { return Option.None; }
            return Option.Some(n);
        }
        fun f(n: int): Result<int, string> {
            let v = first(n)?;
            return Result.Ok(v);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

// -- Class indexer (`obj[i]` / `obj[i] = v`) and enumerator (`for (let x in obj)`) --

#[test]
fn test_class_indexer_get_set_ok() {
    // A class with `@get`/`@set` methods is indexable (method names are free; `get`/`set` still fine).
    let code = "
        class Cell {
            v: int;
            public constructor() { this.v = 0; }
            @get
            public fun get(index: int): int { return this.v + index; }
            @set
            public fun set(index: int, value: int): void { this.v = value; }
        }
        fun main(): void {
            let c = Cell();
            c[1] = 5;
            let x: int = c[2];
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_class_indexer_arbitrary_names_ok() {
    // Protocol role comes from the attribute; method names need not be `get`/`set`.
    let code = "
        class Cell {
            v: int;
            public constructor() { this.v = 0; }
            @get
            public fun at(index: int): int { return this.v + index; }
            @set
            public fun put(index: int, value: int): void { this.v = value; }
        }
        fun main(): void {
            let c = Cell();
            c[1] = 5;
            let x: int = c[2];
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_class_indexer_bare_get_is_not_an_indexer() {
    // Without `@get`, a method named `get` is ordinary — `obj[i]` errors.
    let code = "
        class Box {
            v: int;
            public constructor() { this.v = 0; }
            public fun get(index: int): int { return index; }
        }
        fun main(): void {
            let b = Box();
            let x = b[0];
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("has no indexer")));
}

#[test]
fn test_class_indexer_void_get_attr_rejected() {
    // `@get` returning `void` is rejected at registration.
    let code = "
        class Box {
            v: int;
            public constructor() { this.v = 0; }
            @get
            public fun get(index: int): void { }
        }
        fun main(): void { }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("must return a non-void value")));
}

#[test]
fn test_class_void_get_still_callable_as_method() {
    // Defining a void `get` must NOT break calling it directly as an ordinary method.
    let code = "
        class Box {
            v: int;
            public constructor() { this.v = 0; }
            public fun get(index: int): void { }
        }
        fun main(): void {
            let b = Box();
            b.get(0);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_class_indexer_static_get_attr_rejected() {
    // `@get` on a static method is rejected at registration.
    let code = "
        class Box {
            v: int;
            public constructor() { this.v = 0; }
            @get
            public static fun get(index: int): int { return index; }
        }
        fun main(): void { }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("non-static")));
}

#[test]
fn test_class_indexer_async_get_attr_rejected() {
    // `@get` on an async method is rejected at registration.
    let code = "
        class Box {
            v: int;
            public constructor() { this.v = 0; }
            @get
            public async fun get(index: int): int { return index; }
        }
        fun main(): void { }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("cannot be async")));
}

// -- TypeScript-style property accessors (`get prop()` / `set prop(v)`) --

#[test]
fn test_property_getter_ok() {
    // A well-formed getter/setter pair is read/written via dot access, not brackets.
    let code = "
        class Box {
            v: int;
            public constructor() { this.v = 0; }
            public get value(): int { return this.v; }
            public set value(x: int) { this.v = x; }
        }
        fun main(): void {
            let b = Box();
            b.value = 5;
            let x: int = b.value;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_static_getter_and_setter_ok() {
    // Static accessors are read/written through the type itself: `Counter.count` calls the static
    // getter and `Counter.count = v` calls the static setter (no instance receiver).
    let code = "
        class Counter {
            public static get count(): int { return 42; }
            public static set count(x: int) { }
        }
        fun main(): void {
            Counter.count = 5;
            let n: int = Counter.count;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_static_getter_type_mismatch_is_reported() {
    // A static getter is still type-checked: assigning its `int` result to a `string` errors.
    let code = "
        class Box {
            public static get value(): int { return 0; }
        }
        fun main(): void {
            let s: string = Box.value;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_array_size_builtin_ok() {
    // `arr.size()` is the builtin element-count method on arrays, typed `int` (the same `size()`
    // the stdlib collections expose). Cross-collection consistency is covered by the
    // `size_consistent` e2e case.
    let code = "
        fun main(): void {
            let a = [10, 20, 30];
            let n: int = a.size();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_async_accessor_is_rejected() {
    // An `async` getter would yield a `Future` instead of the property value, so it is rejected.
    let code = "
        class Box {
            v: int;
            public constructor() { this.v = 0; }
            public async get value(): int { return this.v; }
        }
        fun main(): void {
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("cannot be 'async'")));
}

#[test]
fn test_class_foreach_with_option_enumerator_ok() {
    // The full enumerator protocol: `@iterator` returns an object whose `@next(): Option<T>`
    // yields elements. `break`/`continue` are valid in the body.
    let code = "
        enum Option<T> { Some(value: T), None }
        class RangeIter {
            cur: int;
            end: int;
            public constructor(s: int, e: int) { this.cur = s; this.end = e; }
            @next
            public fun next(): Option<int> {
                if (this.cur >= this.end) { return Option.None; }
                let v = this.cur;
                this.cur = this.cur + 1;
                return Option.Some(v);
            }
        }
        class Range {
            start: int;
            end: int;
            public constructor(s: int, e: int) { this.start = s; this.end = e; }
            @iterator
            public fun iterator(): RangeIter { return RangeIter(this.start, this.end); }
        }
        fun main(): void {
            let total = 0;
            for (let x in Range(0, 5)) {
                if (x == 2) { continue; }
                if (x == 4) { break; }
                total = total + x;
            }
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_class_foreach_next_not_option_errors() {
    // `@next` must return `Option<T>`; a non-Option return is rejected at registration.
    let code = "
        class NumIter {
            n: int;
            public constructor() { this.n = 0; }
            @next
            public fun next(): int { return 0; }
        }
        class Nums {
            public constructor() { }
            @iterator
            public fun iterator(): NumIter { return NumIter(); }
        }
        fun main(): void {
            for (let x in Nums()) { }
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("must return Option")));
}

#[test]
fn test_class_foreach_missing_iterator_errors() {
    // A class without an `@iterator` method cannot be iterated with `for..in`.
    let code = "
        class Plain {
            v: int;
            public constructor() { this.v = 0; }
        }
        fun main(): void {
            for (let x in Plain()) { }
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("@iterator")));
}

#[test]
fn test_interface_implemented_ok() {
    // A class providing every interface method with a matching signature analyzes cleanly, and a
    // concrete value flows into an interface-typed local via an implicit upcast, then dispatches.
    let code = "
        interface Animal {
            fun speak(): string;
            fun legs(): int;
        }
        class Cat : Animal {
            public fun speak(): string { return \"meow\"; }
            public fun legs(): int { return 4; }
        }
        fun run(): string {
            let c = Cat();
            let a: Animal = c;
            return a.speak();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_interface_missing_method_errors() {
    // Declaring `: Animal` obliges the class to implement every method of the interface.
    let code = "
        interface Animal {
            fun speak(): string;
            fun legs(): int;
        }
        class Cat : Animal {
            public fun speak(): string { return \"meow\"; }
        }
        fun main(): void { let c = Cat(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not implement method") && d.message.contains("legs")));
}

#[test]
fn test_interface_cannot_be_instantiated() {
    let code = "
        interface Animal { fun speak(): string; }
        fun main(): void { let a = Animal(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("instantiate interface")));
}


#[test]
fn test_generic_interface_monomorphized_ok() {
    // A generic class implementing a generic interface analyzes cleanly, and a call on the
    // monomorphized interface type dispatches dynamically.
    let code = "
        interface Container<T> {
            fun get(): T;
            fun size(): int;
        }
        class Box<T> : Container<T> {
            public value: T;
            public constructor(value: T) { this.value = value; }
            public fun get(): T { return this.value; }
            public fun size(): int { return 1; }
        }
        fun describe(c: Container<int>): int { return c.get(); }
        fun run(): int {
            let b = Box<int>(7);
            return describe(b);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_generic_interface_signature_mismatch_errors() {
    // The class's monomorphized method must match the interface's monomorphized signature.
    let code = "
        interface Container<T> {
            fun get(): T;
        }
        class Box<T> : Container<T> {
            public value: T;
            public fun get(): int { return 0; }
        }
        fun run(): int {
            let b = Box<string>(\"x\");
            return b.size();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not match the signature")
            || d.message.contains("does not implement method")));
}

#[test]
fn test_async_interface_method_ok() {
    // An async interface method implemented by an async class method analyzes cleanly; calling it
    // through an interface-typed receiver yields an awaitable `Future`.
    let code = "
        interface Fetcher { async fun fetch(): int; }
        class Remote : Fetcher {
            public async fun fetch(): int { return 1; }
        }
        async fun run(f: Fetcher): int { return await f.fetch(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_async_interface_method_requires_async_impl() {
    // A sync class method cannot satisfy an async interface method (they compile to different
    // shapes), so the implements check must reject it.
    let code = "
        interface Fetcher { async fun fetch(): int; }
        class Remote : Fetcher {
            public fun fetch(): int { return 1; }
        }
        fun main(): void { let r = Remote(); }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not match the signature")));
}

#[test]
fn test_is_binding_not_visible_outside_branch() {
    // The `is`-with-binding local is scoped to the taken branch; referencing it afterwards is an
    // error.
    let code = "
        fun f(o: object): int {
            if (o is int a) { return a; }
            return a;
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("does not exist")));
}

#[test]
fn test_is_binding_in_and_chain_is_visible_in_branch() {
    // A binding reached through a top-level `&&` chain (`a is Cat c && flag`) is guaranteed to hold
    // in the then-branch, so `c` is in scope there.
    let code = "
        interface Animal { fun speak(): int; }
        class Cat : Animal { public fun speak(): int { return 7; } }
        fun check(a: Animal, flag: bool): int {
            if (a is Cat c && flag) { return c.speak(); }
            return 0;
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        !diagnostics.has_errors(),
        "is-binding via && chain should type-check: {:?}",
        diagnostics.diagnostics
    );
}

#[test]
fn test_is_binding_visible_in_later_and_conjunct() {
    // `t` bound by the first conjunct's `is` is now also visible in a later conjunct of the same
    // `&&` chain (previously only the branch body could see it).
    let code = "
        interface Animal { fun speak(): int; }
        class Cat : Animal { public fun speak(): int { return 7; } }
        fun check(a: Animal): int {
            if (a is Cat t && t.speak() > 0) { return 1; }
            return 0;
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        !diagnostics.has_errors(),
        "is-binding should be visible in a later && conjunct: {:?}",
        diagnostics.diagnostics
    );
}

#[test]
fn test_is_binding_visible_across_multiple_and_conjuncts() {
    // A three-way chain: the binding from the first conjunct must survive being threaded through
    // an intermediate plain-bool conjunct to a third conjunct that uses it.
    let code = "
        interface Animal { fun speak(): int; }
        class Cat : Animal { public fun speak(): int { return 7; } }
        fun check(a: Animal, flag: bool): int {
            if (a is Cat t && flag && t.speak() > 0) { return 1; }
            return 0;
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        !diagnostics.has_errors(),
        "is-binding should survive an intermediate conjunct: {:?}",
        diagnostics.diagnostics
    );
}

#[test]
fn test_is_binding_not_visible_across_or() {
    // `||` does not guarantee the left side held, so a binding from it must stay invisible on the
    // right — this must keep failing after the `&&` fix.
    let code = "
        interface Animal { fun speak(): int; }
        class Cat : Animal { public fun speak(): int { return 7; } }
        fun check(a: Animal): int {
            if (a is Cat t || t.speak() > 0) { return 1; }
            return 0;
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        diagnostics.has_errors(),
        "is-binding must not cross '||' into the right operand"
    );
}

#[test]
fn test_is_binding_in_while_is_visible_in_body() {
    // A `while (a is Cat c)` binding narrows `c` for the loop body (the body only runs when the
    // condition holds), so `c.speak()` resolves.
    let code = "
        interface Animal { fun speak(): int; }
        class Cat : Animal { public fun speak(): int { return 7; } }
        fun drain(a: Animal): int {
            while (a is Cat c) { return c.speak(); }
            return 0;
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        !diagnostics.has_errors(),
        "is-binding in while should type-check: {:?}",
        diagnostics.diagnostics
    );
}

#[test]
fn test_is_binding_from_while_not_visible_after_loop() {
    // The narrowed local is scoped to the loop body only; using it after the loop is an error.
    let code = "
        interface Animal { fun speak(): int; }
        class Cat : Animal { public fun speak(): int { return 7; } }
        fun drain(a: Animal): int {
            while (a is Cat c) { return c.speak(); }
            return c.speak();
        }
    ";
    let diagnostics = analyze_code(code);
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|d| d.message.contains("does not exist")),
        "binding must not leak past the loop: {:?}",
        diagnostics.diagnostics
    );
}

#[test]
fn test_default_param_call_with_and_without_optional_arg() {
    // A trailing default parameter may be supplied or omitted; both calls are well-typed.
    let code = "
        fun greet(name: string, times: int = 1): void {}
        fun main(): void {
            greet(\"hi\");
            greet(\"hi\", 2);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_default_param_missing_required_arg_errors() {
    // The leading required parameter still must be supplied: `greet()` provides fewer than the
    // required count and is an error.
    let code = "
        fun greet(name: string, times: int = 1): void {}
        fun main(): void {
            greet();
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_default_param_too_many_args_errors() {
    // Supplying more than the total parameter count is still an arity error, reported with the
    // range message.
    let code = "
        fun greet(name: string, times: int = 1): void {}
        fun main(): void {
            greet(\"hi\", 1, 2);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("between 1 and 2 arguments")));
}

#[test]
fn test_default_param_after_required_used_in_call() {
    // The default's value is substituted, so a numeric default type-checks against its declared
    // parameter type without error.
    let code = "
        fun scale(base: int, factor: int = 2): int { return base * factor; }
        fun main(): void {
            let a: int = scale(5);
            let b: int = scale(5, 3);
        }
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_default_param_rejected_after_required_at_analysis() {
    // The parser reports a required parameter following a defaulted one; analysis surfaces it too.
    let code = "
        fun bad(x: int = 1, y: int): void {}
        fun main(): void {}
    ";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

// --- dynamic `js` interop -----------------------------------------------------------------------

#[test]
fn test_js_unknown_member_and_method_compile() {
    // `js` uses deferred binding: unknown members/methods of arbitrary arity are legal by
    // construction (no member resolution), so this compiles without error.
    let code = format!(
        "{JS_STUB}
        fun main(): void {{
            let doc = js.global(\"document\");
            let el = doc.getElementById(\"app\");
            el.classList.add(\"a\", \"b\", \"c\");
            el.totallyMadeUpMethod();
        }}"
    );
    let diagnostics = analyze_code(&code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_js_property_set_auto_marshals_primitives() {
    // Writing a Dream primitive/string into a `js` property type-checks (auto-box), and reading a
    // `js` value into a typed binding type-checks (auto-unbox) - no manual conversion needed.
    let code = format!(
        "{JS_STUB}
        fun main(): void {{
            let el = js.global(\"document\").getElementById(\"app\");
            el.textContent = \"hello\";
            el.tabIndex = 3;
            el.hidden = true;
            let n: int = el.childNodes.length;
        }}"
    );
    let diagnostics = analyze_code(&code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_js_call_expression_invokes_value() {
    // A `js`-typed value is callable directly (`cb(...)`), desugaring to the invoke bridge.
    let code = format!(
        "{JS_STUB}
        fun main(): void {{
            let cb = js.global(\"logger\");
            cb(\"done\");
        }}"
    );
    let diagnostics = analyze_code(&code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_js_index_access() {
    // Indexing a `js` value with either a string or int key is legal and yields `js`.
    let code = format!(
        "{JS_STUB}
        fun main(): void {{
            let obj = js.global(\"data\");
            let first = obj[\"items\"];
            obj[0] = \"x\";
        }}"
    );
    let diagnostics = analyze_code(&code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_js_struct_marshaling() {
    // A struct/class deep-copies into a `js` object when passed to / stored in `js`, and a `js`
    // value reconstructs a reference class at a typed binding - both type-check without explicit
    // conversions.
    let code = format!(
        "{JS_STUB}
        class Point {{
            public x: int;
            public y: int;
            public constructor(x: int, y: int) {{ this.x = x; this.y = y; }}
        }}
        fun main(): void {{
            let p = Point(1, 2);
            js.global.send(p);              // struct -> js argument
            let doc = js.global.document;
            doc.origin = p;                 // struct -> js property
            let q: Point = js.global.makePoint();  // js -> struct binding
            let n: int = q.x;
        }}"
    );
    let diagnostics = analyze_code(&code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_js_global_property_syntax() {
    // `js.global` is `globalThis` as a value, so members chain off it: `js.global.document` reads a
    // property and `js.global.fetch(...)` / `js.global.document.getElementById(...)` call through it.
    let code = format!(
        "{JS_STUB}
        fun main(): void {{
            let doc = js.global.document;
            let el = js.global.document.getElementById(\"app\");
            js.global.console.log(\"hi\");
        }}"
    );
    let diagnostics = analyze_code(&code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_js_await_promise() {
    // Awaiting a `js` value (a JS Promise handle) is legal inside an async function and yields
    // `Option<js>` - `Some` on resolve, `None` on rejection.
    let code = format!(
        "{JS_STUB}
        async fun main(): void {{
            let user = await js.global.fetchUser(42);
            let name: string = switch (user) {{
                Some(u) => u.name.to_str(),
                None => \"\",
            }};
        }}"
    );
    let diagnostics = analyze_code(&code);
    assert_eq!(diagnostics.has_errors(), false);
}


#[test]
fn test_extend_sealed_class_is_rejected() {
    // A `sealed` class may not be targeted by a user `extend` block.
    let code = "sealed class Locked { public v: int; public constructor() { this.v = 0; } } \
                extend Locked { public fun bump(): int { return this.v + 1; } }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Cannot extend sealed type 'Locked'")));
}

#[test]
fn test_extend_sealed_enum_is_rejected() {
    let code = "sealed enum Color { Red, Green } \
                extend Color { public fun label(): int { return 0; } }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Cannot extend sealed type 'Color'")));
}

#[test]
fn test_extend_non_sealed_class_is_allowed() {
    // The same extend on a non-sealed class analyzes cleanly (baseline for the sealed rejection).
    let code = "class Open { public v: int; public constructor() { this.v = 0; } } \
                extend Open { public fun bump(): int { return this.v + 1; } }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

// --- `weak`/`unowned` fields and the compile-time reference-cycle check ---

#[test]
fn test_class_self_reference_is_a_cycle_error() {
    // A class holding a strong field of its own type is a self-loop in the reference-cycle
    // graph: it is structurally capable of forming a leak (e.g. `n.next = n`), so it is a hard
    // error even though this particular declaration never actually wires up a loop.
    let code = "class Node { public next: Node; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|d| d.message.contains("reference cycle detected")
                && d.message.contains("Node.next"))
    );
}

#[test]
fn test_class_self_reference_allowed_with_allow_cycle() {
    let code = "@allow_cycle class Node { public next: Node; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_option_wrapped_self_reference_is_a_cycle_error() {
    let code = "enum Option<T> { Some(value: T), None }
        class Node { public next: Option<Node>; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("reference cycle detected")));
}

#[test]
fn test_array_field_self_reference_is_a_cycle_error() {
    // Array elements are strong references, so `Node[]` contributes the same self-loop edge as a
    // bare `Node` field.
    let code = "class Node { public children: Node[]; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("reference cycle detected")));
}

#[test]
fn test_mutual_class_cycle_is_error() {
    let code = "class A { public b: B; }
        class B { public a: A; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("reference cycle detected")
            && d.message.contains("A.b")
            && d.message.contains("B.a")));
}

#[test]
fn test_mutual_cycle_allow_cycle_on_only_one_class_still_errors() {
    // `@allow_cycle` must be present on *every* class participating in the cycle; annotating only
    // one side can't launder a multi-class cycle through it.
    let code = "@allow_cycle class A { public b: B; }
        class B { public a: A; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
}

#[test]
fn test_mutual_cycle_allow_cycle_on_both_suppresses() {
    let code = "@allow_cycle class A { public b: B; }
        @allow_cycle class B { public a: A; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_weak_field_breaks_self_cycle() {
    let code = "enum Option<T> { Some(value: T), None }
        class Node { weak next: Option<Node>; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_unowned_field_breaks_self_cycle() {
    let code = "class Node { unowned next: Node; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}

#[test]
fn test_weak_field_requires_option_type() {
    let code = "class Node { weak next: Node; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("'weak' field") && d.message.contains("Option<T>")));
}

#[test]
fn test_unowned_field_requires_class_type() {
    let code = "enum Option<T> { Some(value: T), None }
        class Node { unowned next: Option<Node>; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("'unowned' field") && d.message.contains("class type")));
}

#[test]
fn test_field_cannot_be_both_weak_and_unowned() {
    let code = "enum Option<T> { Some(value: T), None }
        class Node { weak unowned next: Option<Node>; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), true);
    assert!(diagnostics
        .diagnostics
        .iter()
        .any(|d| d.message.contains("cannot be both 'weak' and 'unowned'")));
}

#[test]
fn test_no_false_positive_for_distinct_parent_child_types() {
    // `Parent` holds a `Child`, but `Child` never references `Parent` back, so there is no cycle.
    let code = "class Child { public name: string; }
        class Parent { public child: Child; }";
    let diagnostics = analyze_code(code);
    assert_eq!(diagnostics.has_errors(), false);
}
