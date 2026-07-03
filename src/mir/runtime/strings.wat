;; String payload layout (at the data pointer `ptr`, i.e. heap block + 12):
;;   [ptr+0]        length : i32
;;   [ptr+4 .. +len] utf8 bytes
;; There is no NUL terminator: the length prefix makes it redundant, and every consumer (strlen,
;; string_eq, hashing, host interop) is length-driven. The 12-byte heap header ([size][tag][ref_count])
;; still lives at ptr-12 and is unchanged, so malloc/free/retain/release/object_tag are unaffected.
;; Length is O(1): a single load at `ptr`.
(func $strlen (param $ptr i32) (result i32)
    local.get $ptr
    i32.load
)

(func $concat_strings (param $str1 i32) (param $str2 i32) (result i32)
    (local $len1 i32)
    (local $len2 i32)
    (local $new_ptr i32)
    (local $i i32)
    local.get $str1
    call $strlen
    local.set $len1
    local.get $str2
    call $strlen
    local.set $len2
    ;; size = 4 (length prefix) + len1 + len2
    local.get $len1
    local.get $len2
    i32.add
    i32.const 4
    i32.add
    i32.const {TAG_STRING}
    call $malloc
    local.set $new_ptr
    ;; store the combined length at [new_ptr]
    local.get $new_ptr
    local.get $len1
    local.get $len2
    i32.add
    i32.store
    ;; copy str1's bytes to new_ptr+4+i
    i32.const 0
    local.set $i
    (block $end1
        (loop $start1
            local.get $i
            local.get $len1
            i32.eq
            br_if $end1
            local.get $new_ptr
            i32.const 4
            i32.add
            local.get $i
            i32.add
            local.get $str1
            i32.const 4
            i32.add
            local.get $i
            i32.add
            i32.load8_u
            i32.store8
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $start1
        )
    )
    ;; copy str2's bytes to new_ptr+4+len1+i
    i32.const 0
    local.set $i
    (block $end2
        (loop $start2
            local.get $i
            local.get $len2
            i32.eq
            br_if $end2
            local.get $new_ptr
            i32.const 4
            i32.add
            local.get $len1
            i32.add
            local.get $i
            i32.add
            local.get $str2
            i32.const 4
            i32.add
            local.get $i
            i32.add
            i32.load8_u
            i32.store8
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $start2
        )
    )
    local.get $new_ptr
)

(func $debug_get_free_list_head (result i32)
    global.get $free_list_head
)

(func $debug_get_heap_ptr (result i32)
    global.get $heap_ptr
)

(func $debug_get_live_objects (result i32)
    global.get $live_objects
)

(func $debug_get_total_allocations (result i32)
    global.get $total_allocations
)

;; Reads the live reference count of a heap value (string/array/struct/object). The data pointer
;; passed in points just past the [size][tag][ref_count] header, so the count lives at ptr-4.
;; A null pointer reports 0.
(func $debug_get_ref_count (param $ptr i32) (result i32)
    local.get $ptr
    i32.eqz
    (if (result i32)
        (then i32.const 0)
        (else
            local.get $ptr
            i32.const 4
            i32.sub
            i32.load
        )
    )
)

(func $string_eq (param $a i32) (param $b i32) (result i32)
    (local $len i32)
    (local $i i32)
    ;; identical pointers (covers the both-null case) are trivially equal
    local.get $a
    local.get $b
    i32.eq
    if
        i32.const 1
        return
    end
    ;; a null pointer can only equal another null pointer (handled above)
    local.get $a
    i32.eqz
    if
        i32.const 0
        return
    end
    local.get $b
    i32.eqz
    if
        i32.const 0
        return
    end
    ;; O(1) length mismatch check before comparing bytes
    local.get $a
    i32.load
    local.set $len
    local.get $len
    local.get $b
    i32.load
    i32.ne
    if
        i32.const 0
        return
    end
    ;; compare the $len char bytes at a+4+i / b+4+i (no NUL sentinel needed)
    i32.const 0
    local.set $i
    (block $done
        (loop $cmp
            local.get $i
            local.get $len
            i32.ge_u
            br_if $done
            local.get $a
            i32.const 4
            i32.add
            local.get $i
            i32.add
            i32.load8_u
            local.get $b
            i32.const 4
            i32.add
            local.get $i
            i32.add
            i32.load8_u
            i32.ne
            if
                i32.const 0
                return
            end
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $cmp
        )
    )
    i32.const 1
)

(func $char_at (param $ptr i32) (param $i i32) (result i32)
    local.get $ptr
    i32.const 4
    i32.add
    local.get $i
    i32.add
    i32.load8_u
)

(func $string_alloc (param $n i32) (result i32)
    (local $p i32)
    ;; 4-byte length prefix + n data bytes (the caller fills the bytes via $string_set)
    local.get $n
    i32.const 4
    i32.add
    i32.const {TAG_STRING}
    call $malloc
    local.set $p
    ;; store the length at [p]
    local.get $p
    local.get $n
    i32.store
    local.get $p
)

(func $string_set (param $ptr i32) (param $i i32) (param $c i32)
    local.get $ptr
    i32.const 4
    i32.add
    local.get $i
    i32.add
    local.get $c
    i32.store8
)
