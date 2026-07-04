;; --- Segregated free-list (slab) allocator -------------------------------------------------------
;;
;; `$malloc`/`$free` are O(1): each request is mapped to a power-of-two size class and served from a
;; per-class free list (no scanning). The only scanned path is the rare "large object" list for
;; requests bigger than the largest size class.
;;
;; Free-list heads live in a fixed, zero-initialized low-memory table (so every list starts empty):
;;   slot i  @  4 + i*4   for i in 0..8  -> size class with block size (1 << (i+4)): 16,32,...,4096
;;   slot 9  @  40                       -> large-object list (blocks larger than 4096 bytes)
;; Low memory [0,1024) is otherwise unused (interned strings, itables and the heap all live >= 1024).
;;
;; Heap block layout is unchanged: [size:i32][tag:i32][ref_count:i32], data at block+12. While a
;; block sits on a free list, block+4 (the tag word) holds the next-free pointer; block+0 keeps the
;; block size, so `$free` can recover the size class in O(1).

;; Maps a total block size (header included) to its size-class index. Returns >= 9 for sizes that
;; exceed the largest class (the "large object" path). Index = ceil(log2(size)) - 4.
;;
;; The size is first clamped up to the smallest block (16 bytes). This keeps the class math correct
;; by construction: for any size < 16, ceil(log2(size)) - 4 would be negative (e.g. 8 bytes ->
;; log2(8) - 4 = -1), which would index before the free-list table and corrupt memory. Clamping to
;; 16 makes the smallest class (index 0) the floor, so a negative index is impossible.
(func $size_class (param $size i32) (result i32)
    (local $s i32)
    local.get $size
    local.set $s
    local.get $s
    i32.const 16
    i32.lt_s
    (if (then i32.const 16 local.set $s))
    ;; idx = ceil(log2(s)) - 4 == (32 - clz(s-1)) - 4, folded to 28 - clz(s-1).
    i32.const 28
    local.get $s
    i32.const 1
    i32.sub
    i32.clz
    i32.sub
)

(func $malloc (param $size i32) (param $tag i32) (result i32)
    (local $idx i32)
    (local $alloc_size i32)
    (local $head_addr i32)
    (local $block i32)
    (local $next i32)
    (local $curr i32)
    (local $prev i32)
    (local $block_size i32)
    (local $new_heap i32)
    ;;@DEBUG_ALLOC_COUNT@
    ;; total block size = payload rounded up to a multiple of 4, plus the 12-byte header
    local.get $size
    i32.const 3
    i32.add
    i32.const -4
    i32.and
    i32.const 12
    i32.add
    local.set $size
    local.get $size
    call $size_class
    local.set $idx

    local.get $idx
    i32.const 8
    i32.gt_s
    (if
        (then
            ;; ---- large object: first-fit over the single large list (slot 9 @ 40) ----
            local.get $size
            local.set $alloc_size
            i32.const 40
            local.set $head_addr
            local.get $head_addr
            i32.load
            local.set $curr
            i32.const 0
            local.set $prev
            (block $large_done
                (loop $large_scan
                    local.get $curr
                    i32.eqz
                    br_if $large_done
                    local.get $curr
                    i32.load
                    local.set $block_size
                    local.get $block_size
                    local.get $size
                    i32.ge_s
                    (if
                        (then
                            ;; unlink `curr` from the large list
                            local.get $curr
                            i32.const 4
                            i32.add
                            i32.load
                            local.set $next
                            local.get $prev
                            i32.eqz
                            (if
                                (then local.get $head_addr local.get $next i32.store)
                                (else local.get $prev i32.const 4 i32.add local.get $next i32.store)
                            )
                            local.get $curr
                            local.set $block
                            br $large_done
                        )
                    )
                    local.get $curr
                    local.set $prev
                    local.get $curr
                    i32.const 4
                    i32.add
                    i32.load
                    local.set $curr
                    br $large_scan
                )
            )
        )
        (else
            ;; ---- small size class: fixed block size, O(1) pop from slot `idx` ----
            i32.const 1
            local.get $idx
            i32.const 4
            i32.add
            i32.shl
            local.set $alloc_size
            local.get $idx
            i32.const 2
            i32.shl
            i32.const 4
            i32.add
            local.set $head_addr
            local.get $head_addr
            i32.load
            local.set $block
            ;; every block on this list is exactly `alloc_size`, so pop the head unconditionally
            local.get $block
            i32.eqz
            i32.eqz
            (if
                (then
                    local.get $block
                    i32.const 4
                    i32.add
                    i32.load
                    local.set $next
                    local.get $head_addr
                    local.get $next
                    i32.store
                )
            )
        )
    )

    ;; no reusable block on the list: bump-allocate a fresh one and record its size
    local.get $block
    i32.eqz
    (if
        (then
            global.get $heap_ptr
            local.set $block
            ;; new bump pointer after this block
            local.get $block
            local.get $alloc_size
            i32.add
            local.set $new_heap
            ;; grow linear memory if the block would run past the currently mapped pages, otherwise
            ;; the store below traps out-of-bounds. Current size in bytes = memory.size << 16.
            (block $have_room
                local.get $new_heap
                memory.size
                i32.const 16
                i32.shl
                i32.le_u
                br_if $have_room
                ;; pages needed = ceil(new_heap / 65536); grow by (needed - current) pages
                local.get $new_heap
                i32.const 1
                i32.sub
                i32.const 16
                i32.shr_u
                i32.const 1
                i32.add
                memory.size
                i32.sub
                memory.grow
                ;; memory.grow yields -1 if the host refuses; nothing sane to do but trap
                i32.const -1
                i32.eq
                (if (then unreachable))
            )
            local.get $new_heap
            global.set $heap_ptr
            local.get $block
            local.get $alloc_size
            i32.store
        )
    )

    ;; write the header (tag at block+4, ref_count=1 at block+8) and return the data pointer
    local.get $block
    i32.const 4
    i32.add
    local.get $tag
    i32.store
    local.get $block
    i32.const 8
    i32.add
    i32.const 1
    i32.store
    local.get $block
    i32.const 12
    i32.add
)

(func $free (param $ptr i32)
    (local $block_start i32)
    (local $idx i32)
    (local $head_addr i32)
    local.get $ptr
    i32.eqz
    br_if 0
    ;;@DEBUG_FREE_COUNT@
    local.get $ptr
    i32.const 12
    i32.sub
    local.set $block_start
    ;; recover the size class from the stored block size (large -> slot 9)
    local.get $block_start
    i32.load
    call $size_class
    local.set $idx
    local.get $idx
    i32.const 8
    i32.gt_s
    (if (then i32.const 9 local.set $idx))
    local.get $idx
    i32.const 2
    i32.shl
    i32.const 4
    i32.add
    local.set $head_addr
    ;; push the block onto its class list: block.next = *head_addr; *head_addr = block
    local.get $block_start
    i32.const 4
    i32.add
    local.get $head_addr
    i32.load
    i32.store
    local.get $head_addr
    local.get $block_start
    i32.store
    ;; mirror the most-recent free for `Debug.free_list_head()` introspection
    local.get $block_start
    global.set $free_list_head
)

(func $retain (param $ptr i32)
    (local $ref_count_ptr i32)
    local.get $ptr
    i32.eqz
    br_if 0
    local.get $ptr
    i32.const 4
    i32.sub
    local.set $ref_count_ptr
    local.get $ref_count_ptr
    local.get $ref_count_ptr
    i32.load
    i32.const 1
    i32.add
    i32.store
)

(func $object_tag (param $ptr i32) (result i32)
    local.get $ptr
    i32.eqz
    (if (result i32)
        (then i32.const 0)
        (else
            local.get $ptr
            i32.const 8
            i32.sub
            i32.load
        )
    )
)

(func $release_generic (param $ptr i32)
    (local $ref_count_ptr i32)
    (local $new_count i32)
    local.get $ptr
    i32.eqz
    br_if 0
    local.get $ptr
    i32.const 4
    i32.sub
    local.set $ref_count_ptr
    local.get $ref_count_ptr
    i32.load
    i32.const 1
    i32.sub
    local.set $new_count
    local.get $ref_count_ptr
    local.get $new_count
    i32.store
    local.get $new_count
    i32.eqz
    (if (then
        local.get $ptr
        call $free
    ))
)
