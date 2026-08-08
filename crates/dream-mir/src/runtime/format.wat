(func $double_to_string (param $v f64) (result i32)
    (local $neg i32)
    (local $micro i64)
    (local $ip i64)
    (local $fr i64)
    (local $ipstr i32)
    (local $buf i32)
    (local $i i32)
    (local $res i32)
    i32.const 0
    local.set $neg
    local.get $v
    f64.const 0
    f64.lt
    (if (then
        i32.const 1
        local.set $neg
        local.get $v
        f64.neg
        local.set $v
    ))
    ;; micro = round(v * 1e6)
    local.get $v
    f64.const 1000000
    f64.mul
    f64.const 0.5
    f64.add
    i64.trunc_f64_s
    local.set $micro
    local.get $micro
    i64.const 1000000
    i64.div_s
    local.set $ip
    local.get $micro
    i64.const 1000000
    i64.rem_s
    local.set $fr
    local.get $ip
    call $long_to_string
    local.set $ipstr
    i32.const 20
    i32.const {TAG_STRING}
    call $malloc
    local.set $buf
    ;; chars live at buf+4 (past the 4-byte length prefix); write '.' at [buf+4]
    local.get $buf
    i32.const 4
    i32.add
    i32.const 46
    i32.store8
    ;; write the 6 fractional digits into (buf+4)[1..6], least-significant last
    i32.const 6
    local.set $i
    (block $wdone
        (loop $wgen
            local.get $i
            i32.const 1
            i32.lt_s
            br_if $wdone
            local.get $buf
            i32.const 4
            i32.add
            local.get $i
            i32.add
            local.get $fr
            i64.const 10
            i64.rem_s
            i32.wrap_i64
            i32.const 48
            i32.add
            i32.store8
            local.get $fr
            i64.const 10
            i64.div_s
            local.set $fr
            local.get $i
            i32.const 1
            i32.sub
            local.set $i
            br $wgen
        )
    )
    ;; trim trailing '0's; $i ends as the cut length (chars to keep in buf)
    i32.const 7
    local.set $i
    (block $tdone
        (loop $tgen
            local.get $i
            i32.const 1
            i32.le_s
            br_if $tdone
            local.get $buf
            i32.const 4
            i32.add
            local.get $i
            i32.const 1
            i32.sub
            i32.add
            i32.load8_u
            i32.const 48
            i32.ne
            br_if $tdone
            local.get $i
            i32.const 1
            i32.sub
            local.set $i
            br $tgen
        )
    )
    ;; if only the '.' is left, drop it too (whole number)
    local.get $i
    i32.const 1
    i32.eq
    (if (then i32.const 0 local.set $i))
    ;; store fractional-part length ($i) at [buf] so $concat_strings sees a valid string
    local.get $buf
    local.get $i
    i32.store
    local.get $ipstr
    local.get $buf
    call $concat_strings
    local.set $res
    local.get $neg
    (if (then
        i32.const {minus}
        local.get $res
        call $concat_strings
        local.set $res
    ))
    local.get $res
)
(func $float_to_string (param $v f32) (result i32)
    local.get $v
    f64.promote_f32
    call $double_to_string
)
