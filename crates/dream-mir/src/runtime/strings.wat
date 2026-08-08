;; String payload layout (at the data pointer `ptr`, i.e. heap block + 12):
;;   [ptr+0]        byte length : i32
;;   [ptr+4 .. +len] utf8 bytes
;; There is no NUL terminator: the length prefix makes it redundant, and every consumer (strlen,
;; string_eq, hashing, host interop) is length-driven. The 12-byte heap header ([size][tag][ref_count])
;; still lives at ptr-12 and is unchanged, so malloc/free/retain/release/object_tag are unaffected.
;; `size()` / `char_at` / iteration use Unicode scalar (code point) indices; `byte_size` / `byte_at`
;; expose raw UTF-8 byte access.

;; Byte length of the UTF-8 payload (O(1)).
(func $str_byte_size (param $ptr i32) (result i32)
    local.get $ptr
    i32.load
)

;; Legacy name kept for concat and other byte-oriented callers.
(func $strlen (param $ptr i32) (result i32)
    local.get $ptr
    call $str_byte_size
)

;; UTF-8 width in bytes of the code point starting at byte offset `off` in `ptr`'s payload.
(func $utf8_width_at (param $ptr i32) (param $off i32) (result i32)
    (local $b i32)
    local.get $ptr
    i32.const 4
    i32.add
    local.get $off
    i32.add
    i32.load8_u
    local.set $b
    ;; ASCII
    local.get $b
    i32.const 0x80
    i32.lt_u
    if
        i32.const 1
        return
    end
    ;; 2-byte lead
    local.get $b
    i32.const 0xE0
    i32.and
    i32.const 0xC0
    i32.eq
    if
        i32.const 2
        return
    end
    ;; 3-byte lead
    local.get $b
    i32.const 0xF0
    i32.and
    i32.const 0xE0
    i32.eq
    if
        i32.const 3
        return
    end
    ;; 4-byte lead (and invalid sequences treated as width 1)
    i32.const 4
)

;; Decodes the Unicode scalar at byte offset `off` in `ptr`'s payload.
(func $utf8_decode_at (param $ptr i32) (param $off i32) (result i32)
    (local $b0 i32)
    (local $b1 i32)
    (local $b2 i32)
    (local $b3 i32)
    (local $base i32)
    local.get $ptr
    i32.const 4
    i32.add
    local.set $base
    local.get $base
    local.get $off
    i32.add
    i32.load8_u
    local.set $b0
    local.get $b0
    i32.const 0x80
    i32.lt_u
    if
        local.get $b0
        return
    end
    local.get $b0
    i32.const 0xE0
    i32.and
    i32.const 0xC0
    i32.eq
    if
        local.get $base
        local.get $off
        i32.const 1
        i32.add
        i32.add
        i32.load8_u
        local.set $b1
        local.get $b0
        i32.const 0x1F
        i32.and
        i32.const 6
        i32.shl
        local.get $b1
        i32.const 0x3F
        i32.and
        i32.or
        return
    end
    local.get $b0
    i32.const 0xF0
    i32.and
    i32.const 0xE0
    i32.eq
    if
        local.get $base
        local.get $off
        i32.const 1
        i32.add
        i32.add
        i32.load8_u
        local.set $b1
        local.get $base
        local.get $off
        i32.const 2
        i32.add
        i32.add
        i32.load8_u
        local.set $b2
        local.get $b0
        i32.const 0x0F
        i32.and
        i32.const 12
        i32.shl
        local.get $b1
        i32.const 0x3F
        i32.and
        i32.const 6
        i32.shl
        i32.or
        local.get $b2
        i32.const 0x3F
        i32.and
        i32.or
        return
    end
    local.get $base
    local.get $off
    i32.const 1
    i32.add
    i32.add
    i32.load8_u
    local.set $b1
    local.get $base
    local.get $off
    i32.const 2
    i32.add
    i32.add
    i32.load8_u
    local.set $b2
    local.get $base
    local.get $off
    i32.const 3
    i32.add
    i32.add
    i32.load8_u
    local.set $b3
    local.get $b0
    i32.const 0x07
    i32.and
    i32.const 18
    i32.shl
    local.get $b1
    i32.const 0x3F
    i32.and
    i32.const 12
    i32.shl
    i32.or
    local.get $b2
    i32.const 0x3F
    i32.and
    i32.const 6
    i32.shl
    i32.or
    local.get $b3
    i32.const 0x3F
    i32.and
    i32.or
)

;; Writes scalar `cp` at byte offset `off` in `ptr`'s payload; returns bytes written.
(func $utf8_encode_at (param $ptr i32) (param $off i32) (param $cp i32) (result i32)
    (local $base i32)
    local.get $ptr
    i32.const 4
    i32.add
    local.set $base
    local.get $cp
    i32.const 0x80
    i32.lt_u
    if
        local.get $base
        local.get $off
        i32.add
        local.get $cp
        i32.store8
        i32.const 1
        return
    end
    local.get $cp
    i32.const 0x800
    i32.lt_u
    if
        local.get $base
        local.get $off
        i32.add
        local.get $cp
        i32.const 6
        i32.shr_u
        i32.const 0xC0
        i32.or
        i32.store8
        local.get $base
        local.get $off
        i32.const 1
        i32.add
        i32.add
        local.get $cp
        i32.const 0x3F
        i32.and
        i32.const 0x80
        i32.or
        i32.store8
        i32.const 2
        return
    end
    local.get $cp
    i32.const 0x10000
    i32.lt_u
    if
        local.get $base
        local.get $off
        i32.add
        local.get $cp
        i32.const 12
        i32.shr_u
        i32.const 0xE0
        i32.or
        i32.store8
        local.get $base
        local.get $off
        i32.const 1
        i32.add
        i32.add
        local.get $cp
        i32.const 6
        i32.shr_u
        i32.const 0x3F
        i32.and
        i32.const 0x80
        i32.or
        i32.store8
        local.get $base
        local.get $off
        i32.const 2
        i32.add
        i32.add
        local.get $cp
        i32.const 0x3F
        i32.and
        i32.const 0x80
        i32.or
        i32.store8
        i32.const 3
        return
    end
    local.get $base
    local.get $off
    i32.add
    local.get $cp
    i32.const 18
    i32.shr_u
    i32.const 0xF0
    i32.or
    i32.store8
    local.get $base
    local.get $off
    i32.const 1
    i32.add
    i32.add
    local.get $cp
    i32.const 12
    i32.shr_u
    i32.const 0x3F
    i32.and
    i32.const 0x80
    i32.or
    i32.store8
    local.get $base
    local.get $off
    i32.const 2
    i32.add
    i32.add
    local.get $cp
    i32.const 6
    i32.shr_u
    i32.const 0x3F
    i32.and
    i32.const 0x80
    i32.or
    i32.store8
    local.get $base
    local.get $off
    i32.const 3
    i32.add
    i32.add
    local.get $cp
    i32.const 0x3F
    i32.and
    i32.const 0x80
    i32.or
    i32.store8
    i32.const 4
)

;; Counts Unicode scalars in the UTF-8 payload.
(func $str_scalar_len (param $ptr i32) (result i32)
    (local $byte_len i32)
    (local $off i32)
    (local $count i32)
    local.get $ptr
    call $str_byte_size
    local.set $byte_len
    i32.const 0
    local.set $off
    i32.const 0
    local.set $count
    (block $done
        (loop $scan
            local.get $off
            local.get $byte_len
            i32.ge_u
            br_if $done
            local.get $off
            local.get $ptr
            local.get $off
            call $utf8_width_at
            i32.add
            local.set $off
            local.get $count
            i32.const 1
            i32.add
            local.set $count
            br $scan
        )
    )
    local.get $count
)

;; Byte offset of scalar index `idx` in `ptr`'s payload; returns `byte_len` when `idx` equals scalar count.
(func $utf8_scalar_byte_offset (param $ptr i32) (param $idx i32) (result i32)
    (local $byte_len i32)
    (local $off i32)
    (local $count i32)
    local.get $ptr
    call $str_byte_size
    local.set $byte_len
    i32.const 0
    local.set $off
    i32.const 0
    local.set $count
    (block $done
        (loop $scan
            local.get $count
            local.get $idx
            i32.eq
            br_if $done
            local.get $off
            local.get $byte_len
            i32.ge_u
            br_if $done
            local.get $off
            local.get $ptr
            local.get $off
            call $utf8_width_at
            i32.add
            local.set $off
            local.get $count
            i32.const 1
            i32.add
            local.set $count
            br $scan
        )
    )
    local.get $off
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
    i32.const {HEAP_PTR_ADDR}
    i32.atomic.load
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

;; Unchecked scalar read; call sites emit a scalar-index bounds check first.
(func $char_at (param $ptr i32) (param $i i32) (result i32)
    (local $off i32)
    local.get $ptr
    local.get $i
    call $utf8_scalar_byte_offset
    local.set $off
    local.get $ptr
    local.get $off
    call $utf8_decode_at
)

;; Unchecked byte read; call sites emit a byte-index bounds check first.
(func $byte_at (param $ptr i32) (param $i i32) (result i32)
    local.get $ptr
    i32.const 4
    i32.add
    local.get $i
    i32.add
    i32.load8_u
)

;; Allocates an empty string buffer with room for up to `n` Unicode scalars (4*n payload bytes).
(func $string_alloc (param $n i32) (result i32)
    (local $p i32)
    local.get $n
    i32.const 4
    i32.mul
    i32.const 4
    i32.add
    i32.const {TAG_STRING}
    call $malloc
    local.set $p
    local.get $p
    i32.const 0
    i32.store
    local.get $p
)

;; Writes scalar `c` at scalar index `i`, appending when `i` equals the current scalar count.
(func $string_set (param $ptr i32) (param $i i32) (param $c i32)
    (local $scalar_len i32)
    (local $byte_off i32)
    (local $old_w i32)
    (local $new_w i32)
    (local $byte_len i32)
    (local $tail i32)
    (local $dst i32)
    (local $src i32)
    local.get $ptr
    call $str_scalar_len
    local.set $scalar_len
    local.get $i
    local.get $scalar_len
    i32.gt_u
    if
        unreachable
    end
    local.get $i
    local.get $scalar_len
    i32.eq
    if
        local.get $ptr
        call $str_byte_size
        local.set $byte_len
        local.get $ptr
        local.get $byte_len
        local.get $c
        call $utf8_encode_at
        local.set $new_w
        local.get $ptr
        local.get $byte_len
        local.get $new_w
        i32.add
        i32.store
        return
    end
    local.get $ptr
    local.get $i
    call $utf8_scalar_byte_offset
    local.set $byte_off
    local.get $ptr
    local.get $byte_off
    call $utf8_width_at
    local.set $old_w
    local.get $ptr
    local.get $byte_off
    local.get $c
    call $utf8_encode_at
    local.set $new_w
    local.get $old_w
    local.get $new_w
    i32.eq
    if
        local.get $ptr
        local.get $byte_off
        local.get $c
        call $utf8_encode_at
        drop
        return
    end
    local.get $ptr
    call $str_byte_size
    local.set $byte_len
    local.get $byte_len
    local.get $byte_off
    i32.sub
    local.get $old_w
    i32.sub
    local.set $tail
    local.get $byte_off
    local.get $new_w
    i32.add
    local.set $dst
    local.get $byte_off
    local.get $old_w
    i32.add
    local.set $src
    (block $shift_done
        (loop $shift
            local.get $tail
            i32.eqz
            br_if $shift_done
            local.get $ptr
            i32.const 4
            i32.add
            local.get $dst
            i32.add
            local.get $ptr
            i32.const 4
            i32.add
            local.get $src
            i32.add
            i32.load8_u
            i32.store8
            local.get $dst
            i32.const 1
            i32.add
            local.set $dst
            local.get $src
            i32.const 1
            i32.add
            local.set $src
            local.get $tail
            i32.const 1
            i32.sub
            local.set $tail
            br $shift
        )
    )
    local.get $ptr
    local.get $byte_off
    local.get $c
    call $utf8_encode_at
    drop
    local.get $ptr
    local.get $byte_len
    local.get $new_w
    i32.add
    local.get $old_w
    i32.sub
    i32.store
)
