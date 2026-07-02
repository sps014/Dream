(func $box_int (param $v i32) (result i32)
    (local $p i32)
    i32.const 4
    i32.const {TAG_INT}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    i32.store
    local.get $p
)
(func $box_float (param $v f32) (result i32)
    (local $p i32)
    i32.const 4
    i32.const {TAG_FLOAT}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    f32.store
    local.get $p
)
(func $box_double (param $v f64) (result i32)
    (local $p i32)
    i32.const 8
    i32.const {TAG_DOUBLE}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    f64.store
    local.get $p
)
(func $box_bool (param $v i32) (result i32)
    (local $p i32)
    i32.const 4
    i32.const {TAG_BOOL}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    i32.store
    local.get $p
)
(func $unbox_int (param $p i32) (result i32)
    local.get $p
    i32.load
)
(func $unbox_float (param $p i32) (result f32)
    local.get $p
    f32.load
)
(func $unbox_double (param $p i32) (result f64)
    local.get $p
    f64.load
)
(func $unbox_bool (param $p i32) (result i32)
    local.get $p
    i32.load
)
(func $hash_int (param $v i32) (result i32)
    local.get $v
)
(func $hash_bool (param $v i32) (result i32)
    local.get $v
)
(func $hash_float (param $v f32) (result i32)
    local.get $v
    i32.reinterpret_f32
)
(func $hash_double (param $v f64) (result i32)
    local.get $v
    f32.demote_f64
    i32.reinterpret_f32
)
(func $hash_string (param $p i32) (result i32)
    (local $h i32)
    (local $i i32)
    (local $len i32)
    (local $c i32)
    i32.const -2128831035
    local.set $h
    ;; length prefix at [p]; chars at p+4
    local.get $p
    i32.load
    local.set $len
    i32.const 0
    local.set $i
    (block $done
        (loop $scan
            local.get $i
            local.get $len
            i32.ge_u
            br_if $done
            local.get $p
            i32.const 4
            i32.add
            local.get $i
            i32.add
            i32.load8_u
            local.set $c
            local.get $h
            local.get $c
            i32.xor
            local.set $h
            local.get $h
            i32.const 16777619
            i32.mul
            local.set $h
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $scan
        )
    )
    local.get $h
)
(func $int_to_string (param $v i32) (result i32)
    (local $p i32)
    (local $d i32)
    (local $i i32)
    (local $neg i32)
    (local $start i32)
    (local $end i32)
    (local $tmp i32)
    (local $digit i32)
    i32.const 20
    i32.const {TAG_STRING}
    call $malloc
    local.set $p
    ;; $d is the char data pointer (past the 4-byte length prefix)
    local.get $p
    i32.const 4
    i32.add
    local.set $d
    local.get $v
    i32.eqz
    (if (then
        local.get $d
        i32.const 48
        i32.store8
        local.get $d
        i32.const 1
        i32.add
        i32.const 0
        i32.store8
        local.get $p
        i32.const 1
        i32.store
        local.get $p
        return
    ))
    i32.const 0
    local.set $neg
    local.get $v
    i32.const 0
    i32.lt_s
    (if (then
        i32.const 1
        local.set $neg
        i32.const 0
        local.get $v
        i32.sub
        local.set $v
    ))
    i32.const 0
    local.set $i
    (block $gen_done
        (loop $gen
            local.get $v
            i32.eqz
            br_if $gen_done
            local.get $v
            i32.const 10
            i32.rem_s
            local.set $digit
            local.get $d
            local.get $i
            i32.add
            local.get $digit
            i32.const 48
            i32.add
            i32.store8
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            local.get $v
            i32.const 10
            i32.div_s
            local.set $v
            br $gen
        )
    )
    local.get $neg
    (if (then
        local.get $d
        local.get $i
        i32.add
        i32.const 45
        i32.store8
        local.get $i
        i32.const 1
        i32.add
        local.set $i
    ))
    local.get $d
    local.get $i
    i32.add
    i32.const 0
    i32.store8
    local.get $p
    local.get $i
    i32.store
    i32.const 0
    local.set $start
    local.get $i
    i32.const 1
    i32.sub
    local.set $end
    (block $rev_done
        (loop $rev
            local.get $start
            local.get $end
            i32.ge_s
            br_if $rev_done
            local.get $d
            local.get $start
            i32.add
            i32.load8_u
            local.set $tmp
            local.get $d
            local.get $start
            i32.add
            local.get $d
            local.get $end
            i32.add
            i32.load8_u
            i32.store8
            local.get $d
            local.get $end
            i32.add
            local.get $tmp
            i32.store8
            local.get $start
            i32.const 1
            i32.add
            local.set $start
            local.get $end
            i32.const 1
            i32.sub
            local.set $end
            br $rev
        )
    )
    local.get $p
)
(func $box_char (param $v i32) (result i32)
    (local $p i32)
    i32.const 4
    i32.const {TAG_CHAR}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    i32.store
    local.get $p
)
(func $unbox_char (param $p i32) (result i32)
    local.get $p
    i32.load
)
(func $char_to_string (param $v i32) (result i32)
    (local $p i32)
    i32.const 6
    i32.const {TAG_STRING}
    call $malloc
    local.set $p
    ;; length = 1
    local.get $p
    i32.const 1
    i32.store
    ;; char at p+4
    local.get $p
    i32.const 4
    i32.add
    local.get $v
    i32.store8
    ;; null terminator at p+5
    local.get $p
    i32.const 5
    i32.add
    i32.const 0
    i32.store8
    local.get $p
)
;; ----- New integer primitives: long (i64), ulong (i64), uint (i32), byte (i32) -----
;; `byte`/`uint` box into a 4-byte block (TAG_BYTE=11 / TAG_UINT=9); `long`/`ulong` into an
;; 8-byte block (TAG_LONG=8 / TAG_ULONG=10).
(func $box_byte (param $v i32) (result i32)
    (local $p i32)
    i32.const 4
    i32.const {TAG_BYTE}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    i32.store
    local.get $p
)
(func $box_uint (param $v i32) (result i32)
    (local $p i32)
    i32.const 4
    i32.const {TAG_UINT}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    i32.store
    local.get $p
)
(func $box_long (param $v i64) (result i32)
    (local $p i32)
    i32.const 8
    i32.const {TAG_LONG}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    i64.store
    local.get $p
)
(func $box_ulong (param $v i64) (result i32)
    (local $p i32)
    i32.const 8
    i32.const {TAG_ULONG}
    call $malloc
    local.set $p
    local.get $p
    local.get $v
    i64.store
    local.get $p
)
(func $unbox_byte (param $p i32) (result i32)
    local.get $p
    i32.load
)
(func $unbox_uint (param $p i32) (result i32)
    local.get $p
    i32.load
)
(func $unbox_long (param $p i32) (result i64)
    local.get $p
    i64.load
)
(func $unbox_ulong (param $p i32) (result i64)
    local.get $p
    i64.load
)
;; A `byte` is always in [0, 255], so signed `$int_to_string` renders it correctly.
(func $byte_to_string (param $v i32) (result i32)
    local.get $v
    call $int_to_string
)
;; A `uint` may exceed i32's signed range, so render it through the unsigned 64-bit formatter.
(func $uint_to_string (param $v i32) (result i32)
    local.get $v
    i64.extend_i32_u
    call $ulong_to_string
)
;; hash of a 64-bit value: fold the high and low 32-bit words together.
(func $hash_long (param $v i64) (result i32)
    local.get $v
    i32.wrap_i64
    local.get $v
    i64.const 32
    i64.shr_u
    i32.wrap_i64
    i32.xor
)
;; Signed 64-bit decimal formatter (mirrors $int_to_string with i64 arithmetic).
(func $long_to_string (param $v i64) (result i32)
    (local $p i32)
    (local $d i32)
    (local $i i32)
    (local $neg i32)
    (local $start i32)
    (local $end i32)
    (local $tmp i32)
    (local $digit i32)
    i32.const 28
    i32.const {TAG_STRING}
    call $malloc
    local.set $p
    local.get $p
    i32.const 4
    i32.add
    local.set $d
    local.get $v
    i64.eqz
    (if (then
        local.get $d
        i32.const 48
        i32.store8
        local.get $d
        i32.const 1
        i32.add
        i32.const 0
        i32.store8
        local.get $p
        i32.const 1
        i32.store
        local.get $p
        return
    ))
    i32.const 0
    local.set $neg
    local.get $v
    i64.const 0
    i64.lt_s
    (if (then
        i32.const 1
        local.set $neg
        i64.const 0
        local.get $v
        i64.sub
        local.set $v
    ))
    i32.const 0
    local.set $i
    (block $gen_done
        (loop $gen
            local.get $v
            i64.eqz
            br_if $gen_done
            local.get $v
            i64.const 10
            i64.rem_s
            i32.wrap_i64
            local.set $digit
            local.get $d
            local.get $i
            i32.add
            local.get $digit
            i32.const 48
            i32.add
            i32.store8
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            local.get $v
            i64.const 10
            i64.div_s
            local.set $v
            br $gen
        )
    )
    local.get $neg
    (if (then
        local.get $d
        local.get $i
        i32.add
        i32.const 45
        i32.store8
        local.get $i
        i32.const 1
        i32.add
        local.set $i
    ))
    local.get $d
    local.get $i
    i32.add
    i32.const 0
    i32.store8
    local.get $p
    local.get $i
    i32.store
    i32.const 0
    local.set $start
    local.get $i
    i32.const 1
    i32.sub
    local.set $end
    (block $rev_done
        (loop $rev
            local.get $start
            local.get $end
            i32.ge_s
            br_if $rev_done
            local.get $d
            local.get $start
            i32.add
            i32.load8_u
            local.set $tmp
            local.get $d
            local.get $start
            i32.add
            local.get $d
            local.get $end
            i32.add
            i32.load8_u
            i32.store8
            local.get $d
            local.get $end
            i32.add
            local.get $tmp
            i32.store8
            local.get $start
            i32.const 1
            i32.add
            local.set $start
            local.get $end
            i32.const 1
            i32.sub
            local.set $end
            br $rev
        )
    )
    local.get $p
)
;; Unsigned 64-bit decimal formatter (no sign handling; unsigned div/rem).
(func $ulong_to_string (param $v i64) (result i32)
    (local $p i32)
    (local $d i32)
    (local $i i32)
    (local $start i32)
    (local $end i32)
    (local $tmp i32)
    (local $digit i32)
    i32.const 28
    i32.const {TAG_STRING}
    call $malloc
    local.set $p
    local.get $p
    i32.const 4
    i32.add
    local.set $d
    local.get $v
    i64.eqz
    (if (then
        local.get $d
        i32.const 48
        i32.store8
        local.get $d
        i32.const 1
        i32.add
        i32.const 0
        i32.store8
        local.get $p
        i32.const 1
        i32.store
        local.get $p
        return
    ))
    i32.const 0
    local.set $i
    (block $gen_done
        (loop $gen
            local.get $v
            i64.eqz
            br_if $gen_done
            local.get $v
            i64.const 10
            i64.rem_u
            i32.wrap_i64
            local.set $digit
            local.get $d
            local.get $i
            i32.add
            local.get $digit
            i32.const 48
            i32.add
            i32.store8
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            local.get $v
            i64.const 10
            i64.div_u
            local.set $v
            br $gen
        )
    )
    local.get $d
    local.get $i
    i32.add
    i32.const 0
    i32.store8
    local.get $p
    local.get $i
    i32.store
    i32.const 0
    local.set $start
    local.get $i
    i32.const 1
    i32.sub
    local.set $end
    (block $rev_done
        (loop $rev
            local.get $start
            local.get $end
            i32.ge_s
            br_if $rev_done
            local.get $d
            local.get $start
            i32.add
            i32.load8_u
            local.set $tmp
            local.get $d
            local.get $start
            i32.add
            local.get $d
            local.get $end
            i32.add
            i32.load8_u
            i32.store8
            local.get $d
            local.get $end
            i32.add
            local.get $tmp
            i32.store8
            local.get $start
            i32.const 1
            i32.add
            local.set $start
            local.get $end
            i32.const 1
            i32.sub
            local.set $end
            br $rev
        )
    )
    local.get $p
)
