;; Runtime panic: the single, shared halt-with-message point for every language-level runtime
;; failure (array/string bounds checks, division by zero, bad object-unbox casts, null-reference
;; dereference, and the user-callable `panic(msg)` builtin). Prints `msg` (a Dream string pointer,
;; already fully built by the caller via ordinary string concatenation) followed by a newline using
;; the always-imported `$print_string`/`$print_char` host functions, then traps unconditionally so
;; the module halts deterministically. There is no recovery path: this is a Rust-`panic!`-style
;; fatal error, not a catchable exception.
(func $dream_panic (param $msg i32)
    local.get $msg
    call $print_string
    i32.const 10
    call $print_char
    unreachable
)
