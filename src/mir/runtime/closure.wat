;; --- fun(...) value ABI: a boxed 2-word `[funcidx][env]` heap block --------------------------
;;
;; Every `fun(...)`-typed value (a bare function reference or a lambda, capturing or not) is this
;; box, not a raw table index: `env` is 0 for anything non-capturing, or a capturing lambda's
;; environment object (an `object`-typed pointer, reinterpreted as `i32`) otherwise. An indirect
;; call through a `fun(...)` value unboxes it: `$__closure_env` is set from the box's `env` word
;; (read by the lifted callee's prologue, which reinterprets it back to its own environment
;; struct type — see `analyze_lambda`/`hir_set_indirect_call_through_box`), then the call proceeds
;; through the box's `funcidx` word via the ordinary `call_indirect`. This keeps `call_indirect`'s
;; signature completely unchanged from the pre-closures ABI.
;;
;; The box is deliberately never released (nor is a capturing lambda's environment object, kept
;; alive by exactly one extra, permanent `$retain` at construction time — see `__Closure.retain`):
;; a v1 tradeoff that trades a leak-per-closure-created for never risking a captured environment
;; being freed out from under a closure that outlives its creating function's stack frame.
(func $funcbox_new (param $funcidx i32) (param $env i32) (result i32)
    (local $box i32)
    i32.const 8
    i32.const 0
    call $malloc
    local.set $box
    local.get $box
    local.get $funcidx
    i32.store
    local.get $box
    i32.const 4
    i32.add
    local.get $env
    i32.store
    local.get $box
)

(func $funcbox_funcidx (param $box i32) (result i32)
    local.get $box
    i32.load
)

(func $funcbox_env (param $box i32) (result i32)
    local.get $box
    i32.const 4
    i32.add
    i32.load
)
