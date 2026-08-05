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
        @js(\"Dream\", \"jsCallV\")
        static extern fun call(target: js, name: string, args: js[]): js;
        @js(\"Dream\", \"jsInvokeV\")
        static extern fun invoke(target: js, args: js[]): js;
        @js(\"Dream\", \"jsIndexGetV\")
        static extern fun index_get(target: js, key: js): js;
        @js(\"Dream\", \"jsIndexSetV\")
        static extern fun index_set(target: js, key: js, value: js): void;
        @js(\"Dream\", \"jsAwait\")
        static extern async fun await_promise(target: js): js;
        @js(\"Dream\", \"jsAsInt\")
        static extern fun as_int(target: js): int;
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
