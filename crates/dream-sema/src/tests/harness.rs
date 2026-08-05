//! Shared test harness for analyzer unit tests: `analyze_code` plus reusable Dream source stubs.

use super::super::*;
use dream_diagnostics::DiagnosticBag;
use dream_syntax::lexer::Lexer;
use dream_syntax::parser::Parser;

pub(super) fn analyze_code(code: &str) -> DiagnosticBag {
    let mut diagnostics = DiagnosticBag::new(None);
    let lexer = Lexer::new(code.to_string());
    let arena = bumpalo::Bump::new();
    let mut parser = Parser::new(lexer, &arena, &mut diagnostics);

    if let Ok(tree) = parser.parse() {
        let arena = bumpalo::Bump::new();
        let mut analyzer = Analyzer::new(&tree, &arena);
        let _ = analyzer.analyze(&mut diagnostics);
    }

    diagnostics
}

/// The dynamic-`js` bridge surface (mirrors `stdlib/core/js.dream`), inlined so the interop tests do
/// not depend on the full prelude being merged by the unit-test harness.
pub(super) const JS_STUB: &str = "
    enum Option<T> {
        Some(value: T),
        None,
    }
    extend js {
        @js(\"Dream\", \"jsGlobal\")
        static extern fun global(name: string): js;
        @js(\"Dream\", \"jsGlobalThis\")
        static extern fun global_this(): js;
        @js(\"Dream\", \"jsObject\")
        static extern fun object(): js;
        @js(\"Dream\", \"jsArray\")
        static extern fun array(): js;
        @js(\"Dream\", \"jsFunc\")
        static extern fun func(handler: fun(js): void): js;
        @js(\"Dream\", \"jsFunc0\")
        static extern fun func0(handler: fun(): void): js;
        @js(\"Dream\", \"jsInt\")
        static extern fun box_int(value: int): js;
        @js(\"Dream\", \"jsLong\")
        static extern fun box_long(value: long): js;
        @js(\"Dream\", \"jsDouble\")
        static extern fun box_double(value: double): js;
        @js(\"Dream\", \"jsBool\")
        static extern fun box_bool(value: bool): js;
        @js(\"Dream\", \"jsString\")
        static extern fun box_string(value: string): js;
        @js(\"Dream\", \"jsGetV\")
        static extern fun get(target: js, name: string): js;
        @js(\"Dream\", \"jsSetV\")
        static extern fun set(target: js, name: string, value: js): void;
        @js(\"Dream\", \"jsSetSlot\")
        static extern fun set_slot(target: js, name: string, args_ptr: int, argc: int): void;
        @js(\"Dream\", \"jsCallV\")
        static extern fun call(target: js, name: string, args: js[]): js;
        @js(\"Dream\", \"jsInvokeV\")
        static extern fun invoke(target: js, args: js[]): js;
        @js(\"Dream\", \"jsGetCallV\")
        static extern fun get_call(target: js, prop: string, method: string, args_ptr: int, argc: int): js;
        @js(\"Dream\", \"jsIndexGetV\")
        static extern fun index_get(target: js, key: js): js;
        @js(\"Dream\", \"jsIndexSetV\")
        static extern fun index_set(target: js, key: js, value: js): void;
        @js(\"Dream\", \"jsIndexSetSlot\")
        static extern fun index_set_slot(target: js, args_ptr: int, argc: int): void;
        @js(\"Dream\", \"jsGetAsInt\")
        static extern fun get_as_int(target: js, name: string): int;
        @js(\"Dream\", \"jsGetAsLong\")
        static extern fun get_as_long(target: js, name: string): long;
        @js(\"Dream\", \"jsGetAsDouble\")
        static extern fun get_as_double(target: js, name: string): double;
        @js(\"Dream\", \"jsGetAsBool\")
        static extern fun get_as_bool(target: js, name: string): bool;
        @js(\"Dream\", \"jsGetAsString\")
        static extern fun get_as_string(target: js, name: string): string;
        @js(\"Dream\", \"jsCallAsInt\")
        static extern fun call_as_int(target: js, name: string, args_ptr: int, argc: int): int;
        @js(\"Dream\", \"jsCallAsLong\")
        static extern fun call_as_long(target: js, name: string, args_ptr: int, argc: int): long;
        @js(\"Dream\", \"jsCallAsDouble\")
        static extern fun call_as_double(target: js, name: string, args_ptr: int, argc: int): double;
        @js(\"Dream\", \"jsCallAsBool\")
        static extern fun call_as_bool(target: js, name: string, args_ptr: int, argc: int): bool;
        @js(\"Dream\", \"jsCallAsString\")
        static extern fun call_as_string(target: js, name: string, args_ptr: int, argc: int): string;
        @js(\"Dream\", \"jsInvokeAsInt\")
        static extern fun invoke_as_int(target: js, args_ptr: int, argc: int): int;
        @js(\"Dream\", \"jsInvokeAsLong\")
        static extern fun invoke_as_long(target: js, args_ptr: int, argc: int): long;
        @js(\"Dream\", \"jsInvokeAsDouble\")
        static extern fun invoke_as_double(target: js, args_ptr: int, argc: int): double;
        @js(\"Dream\", \"jsInvokeAsBool\")
        static extern fun invoke_as_bool(target: js, args_ptr: int, argc: int): bool;
        @js(\"Dream\", \"jsInvokeAsString\")
        static extern fun invoke_as_string(target: js, args_ptr: int, argc: int): string;
        @js(\"Dream\", \"jsGetCallAsInt\")
        static extern fun get_call_as_int(target: js, prop: string, method: string, args_ptr: int, argc: int): int;
        @js(\"Dream\", \"jsGetCallAsLong\")
        static extern fun get_call_as_long(target: js, prop: string, method: string, args_ptr: int, argc: int): long;
        @js(\"Dream\", \"jsGetCallAsDouble\")
        static extern fun get_call_as_double(target: js, prop: string, method: string, args_ptr: int, argc: int): double;
        @js(\"Dream\", \"jsGetCallAsBool\")
        static extern fun get_call_as_bool(target: js, prop: string, method: string, args_ptr: int, argc: int): bool;
        @js(\"Dream\", \"jsGetCallAsString\")
        static extern fun get_call_as_string(target: js, prop: string, method: string, args_ptr: int, argc: int): string;
        @js(\"Dream\", \"jsAwait\")
        static extern async fun await_promise(target: js): js;
        @js(\"Dream\", \"jsAsInt\")
        static extern fun as_int(target: js): int;
        @js(\"Dream\", \"jsAsLong\")
        static extern fun as_long(target: js): long;
        @js(\"Dream\", \"jsAsDouble\")
        static extern fun as_double(target: js): double;
        @js(\"Dream\", \"jsAsBool\")
        static extern fun as_bool(target: js): bool;
        @js(\"Dream\", \"jsAsString\")
        static extern fun as_string(target: js): string;
        public fun to_int(): int { return js.as_int(this); }
        public fun to_str(): string { return js.as_string(this); }
    }
";
