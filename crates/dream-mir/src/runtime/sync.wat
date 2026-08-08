;; --- Cross-thread synchronization: thread ids, the reentrant lock-word primitive, atomic retain ---
;;
;; Backs `@shared class`'s embedded lock word, the `lock (obj) { ... }` statement, and `Lock`
;; (`src/stdlib/core/sync.dream`). Every one of these is, at the WAT level, just "acquire/release
;; the reentrant lock word at address `$addr`" — a `lock` statement's target address is `obj_ptr +
;; layout.size` (see `src/mir/abi.rs`'s `@shared class` header-extension note); `Lock` is simply an
;; `@shared class` with no fields of its own, so its lock word sits at `obj_ptr + 0`.

;; Returns a small, dense id unique to the calling thread (1, 2, 3, ...), assigned once (from the
;; shared `THREAD_ID_COUNTER_ADDR` atomic counter) and cached in the per-*instance* `$__tid` global
;; thereafter — every instance of this module (the owner + every `WebWorker`) is a distinct WASM
;; instance with its own `$__tid`, so this is naturally a one-id-per-OS-thread scheme with no host
;; import needed.
(func $__thread_id (result i32)
    (local $t i32)
    global.get $__tid
    local.set $t
    local.get $t
    i32.eqz
    (if
        (then
            i32.const {THREAD_ID_COUNTER_ADDR}
            i32.const 1
            i32.atomic.rmw.add
            i32.const 1
            i32.add
            local.set $t
            local.get $t
            global.set $__tid
        )
    )
    local.get $t
)

;; Acquires the reentrant lock word at `$addr`: `0` means free; otherwise packed as `(owner_tid <<
;; 16) | depth`. A thread not already holding the lock spins (`i32.atomic.rmw.cmpxchg`) until a
;; 0->(tid,1) exchange succeeds; the thread already holding it just bumps `depth` (only that one
;; thread can ever observe `owner == its own tid`, so the increment needs no further
;; synchronization beyond the atomic RMW itself).
(func $__lock_acquire (param $addr i32)
    (local $tid i32)
    (local $cur i32)
    call $__thread_id
    local.set $tid
    (block $done
        (loop $retry
            local.get $addr
            i32.atomic.load
            local.set $cur
            local.get $cur
            i32.eqz
            (if
                (then
                    local.get $addr
                    i32.const 0
                    local.get $tid
                    i32.const 16
                    i32.shl
                    i32.const 1
                    i32.or
                    i32.atomic.rmw.cmpxchg
                    i32.const 0
                    i32.eq
                    br_if $done
                )
                (else
                    local.get $cur
                    i32.const 16
                    i32.shr_u
                    local.get $tid
                    i32.eq
                    (if
                        (then
                            local.get $addr
                            i32.const 1
                            i32.atomic.rmw.add
                            drop
                            br $done
                        )
                    )
                )
            )
            br $retry
        )
    )
)

;; Releases one level of the reentrant lock word at `$addr`, acquired via `$__lock_acquire`: at
;; depth 1 (the outermost acquire), clears the word to `0` (free); otherwise just decrements
;; `depth`. Undefined (a caller bug, not guarded here — matches C# `Monitor`/`lock`'s own contract)
;; if called by a thread that does not currently hold the lock.
(func $__lock_release (param $addr i32)
    (local $cur i32)
    local.get $addr
    i32.atomic.load
    local.set $cur
    local.get $cur
    i32.const 65535
    i32.and
    i32.const 1
    i32.eq
    (if
        (then
            local.get $addr
            i32.const 0
            i32.atomic.store
        )
        (else
            local.get $addr
            i32.const 1
            i32.atomic.rmw.sub
            drop
        )
    )
)

;; Atomic counterpart of `$retain` (`runtime/allocator.wat`), used for `@shared class` instances,
;; which may be retained/released concurrently from more than one thread once a reference has
;; crossed into a `WebWorker` closure.
(func $retain_shared (param $ptr i32)
    local.get $ptr
    i32.eqz
    br_if 0
    local.get $ptr
    i32.const 4
    i32.sub
    i32.const 1
    i32.atomic.rmw.add
    drop
)

;; `Lock.acquire()`/`Lock.release()` (`src/stdlib/core/sync.dream`): `Lock` is an `@shared class`
;; with no fields of its own, so its embedded lock word sits at `this + 0` — these are thin
;; wrappers exposing `$__lock_acquire`/`$__lock_release` as callable methods (for critical sections
;; that span more than one statement/method, unlike the `lock (obj) { ... }` block sugar).
(func $shared_lock_acquire (param $lock i32)
    local.get $lock
    call $__lock_acquire
)
(func $shared_lock_release (param $lock i32)
    local.get $lock
    call $__lock_release
)

;; `Semaphore.acquire()`/`Semaphore.release()`: a classic counting semaphore, independent of the
;; reentrant lock-word scheme above. `Semaphore`'s one field (`permits`, at `this + 0`) is the
;; permit count itself: `acquire` spins a CAS loop decrementing it once it observes a positive
;; count; `release` is a plain atomic increment (no thread ever needs to wait for a `release`).
(func $shared_semaphore_acquire (param $sem i32)
    (local $cur i32)
    (block $done
        (loop $retry
            local.get $sem
            i32.atomic.load
            local.set $cur
            local.get $cur
            i32.const 0
            i32.gt_s
            (if
                (then
                    local.get $sem
                    local.get $cur
                    local.get $cur
                    i32.const 1
                    i32.sub
                    i32.atomic.rmw.cmpxchg
                    local.get $cur
                    i32.eq
                    br_if $done
                )
            )
            br $retry
        )
    )
)
(func $shared_semaphore_release (param $sem i32)
    local.get $sem
    i32.const 1
    i32.atomic.rmw.add
    drop
)
