;; --- `weak`/`unowned` side table -----------------------------------------------------------------
;;
;; A `weak`/`unowned` field never contributes to its referent's strong reference count (see
;; `docs/language/memory.md`), so ARC's ordinary retain/release bookkeeping cannot reset it when the
;; referent is freed. Instead, every live `weak`/`unowned` slot registers itself here (keyed by the
;; referent it currently points at); when that referent's strong count reaches zero, `$weak_clear_all`
;; walks the registrations for it and poisons every slot before it is freed, so a later read observes
;; `None` (`weak`) or traps (`unowned`) instead of a dangling pointer.
;;
;; The table is one unbucketed singly linked list of small heap nodes (private allocations, tag 0,
;; never touched by `$retain`/`$release_*`/`$release_object` — they are managed exclusively by the
;; three functions below via direct `$malloc`/`$free`). This is O(n) per operation, which is the right
;; trade-off here: `weak`/`unowned` fields are rare (this table exists purely to break reference
;; cycles), so a hash table would add complexity without a measurable win.
;;
;; Node layout (20 bytes, `$malloc(20, 0)`):
;;   +0  target : i32   -- the referent this registration watches
;;   +4  slot   : i32   -- where to write on poison: for `weak`, the address of the private weak-box's
;;                         discriminant word (see below); for `unowned`, the field's own address
;;   +8  kind   : i32   -- 0 = weak, 1 = unowned
;;   +12 extra  : i32   -- `weak` only: the `Option<T>` union's `None` discriminant, written to `slot`
;;                         on poison (payload at `slot+4` is zeroed alongside it); unused for `unowned`
;;   +16 next   : i32   -- next node, or 0

(global $weak_list_head (mut i32) (i32.const 0))

;; Registers `slot` as watching `target`. A no-op when `target` is null (an unset/`None` field has
;; nothing to watch). Called once per store into a live `weak`/`unowned` field.
(func $weak_register (param $target i32) (param $slot i32) (param $kind i32) (param $extra i32)
    (local $node i32)
    local.get $target
    i32.eqz
    br_if 0
    i32.const 20
    i32.const 0
    call $malloc
    local.set $node
    local.get $node
    local.get $target
    i32.store
    local.get $node
    i32.const 4
    i32.add
    local.get $slot
    i32.store
    local.get $node
    i32.const 8
    i32.add
    local.get $kind
    i32.store
    local.get $node
    i32.const 12
    i32.add
    local.get $extra
    i32.store
    local.get $node
    i32.const 16
    i32.add
    global.get $weak_list_head
    i32.store
    local.get $node
    global.set $weak_list_head
)

;; Removes the (unique) registration for `(target, slot)`, if any, and frees its node. Called before a
;; `weak`/`unowned` slot is overwritten or torn down, so a stale registration never outlives the slot
;; it watches (which could otherwise poison unrelated memory reused for something else later).
(func $weak_unregister (param $target i32) (param $slot i32)
    (local $prev i32)
    (local $curr i32)
    (local $next i32)
    local.get $target
    i32.eqz
    br_if 0
    i32.const 0
    local.set $prev
    global.get $weak_list_head
    local.set $curr
    (block $done
        (loop $scan
            local.get $curr
            i32.eqz
            br_if $done
            local.get $curr
            i32.const 16
            i32.add
            i32.load
            local.set $next
            local.get $curr
            i32.load
            local.get $target
            i32.ne
            (if
                (then
                    local.get $curr
                    local.set $prev
                    local.get $next
                    local.set $curr
                    br $scan
                )
            )
            local.get $curr
            i32.const 4
            i32.add
            i32.load
            local.get $slot
            i32.ne
            (if
                (then
                    local.get $curr
                    local.set $prev
                    local.get $next
                    local.set $curr
                    br $scan
                )
            )
            local.get $prev
            i32.eqz
            (if
                (then
                    local.get $next
                    global.set $weak_list_head
                )
                (else
                    local.get $prev
                    i32.const 16
                    i32.add
                    local.get $next
                    i32.store
                )
            )
            local.get $curr
            call $free
            br $done
        )
    )
)

;; Called from every generated `$release_<Class>`, right before the object is freed: poisons every
;; live `weak`/`unowned` slot that watches `target` (there may be more than one), unregistering and
;; freeing each watch node as it goes. A no-op when nothing watches `target` (the common case).
(func $weak_clear_all (param $target i32)
    (local $prev i32)
    (local $curr i32)
    (local $next i32)
    (local $slot i32)
    (local $kind i32)
    local.get $target
    i32.eqz
    br_if 0
    i32.const 0
    local.set $prev
    global.get $weak_list_head
    local.set $curr
    (block $done
        (loop $scan
            local.get $curr
            i32.eqz
            br_if $done
            local.get $curr
            i32.const 16
            i32.add
            i32.load
            local.set $next
            local.get $curr
            i32.load
            local.get $target
            i32.eq
            (if
                (then
                    local.get $curr
                    i32.const 4
                    i32.add
                    i32.load
                    local.set $slot
                    local.get $curr
                    i32.const 8
                    i32.add
                    i32.load
                    local.set $kind
                    local.get $kind
                    i32.eqz
                    (if
                        (then
                            ;; weak: `slot` is the private weak-box's discriminant word; reset it to
                            ;; `None` and zero the payload word right after it.
                            local.get $slot
                            local.get $curr
                            i32.const 12
                            i32.add
                            i32.load
                            i32.store
                            local.get $slot
                            i32.const 4
                            i32.add
                            i32.const 0
                            i32.store
                        )
                        (else
                            ;; unowned: `slot` is the field's own address; poison it directly.
                            local.get $slot
                            i32.const 0
                            i32.store
                        )
                    )
                    local.get $prev
                    i32.eqz
                    (if
                        (then
                            local.get $next
                            global.set $weak_list_head
                        )
                        (else
                            local.get $prev
                            i32.const 16
                            i32.add
                            local.get $next
                            i32.store
                        )
                    )
                    local.get $curr
                    call $free
                )
                (else
                    local.get $curr
                    local.set $prev
                )
            )
            local.get $next
            local.set $curr
            br $scan
        )
    )
)
