(type $dream_poll_t (func (param i32) (result i32)))
(global $rq_head (mut i32) (i32.const 0))
(global $rq_tail (mut i32) (i32.const 0))
(global $timer_head (mut i32) (i32.const 0))
(global $vclock (mut i32) (i32.const 0))
(func $dream_new_future (param $size i32) (param $poll i32) (param $kind i32) (result i32)
    (local $p i32)
    local.get $size
    i32.const 0
    call $malloc
    local.set $p
    local.get $p
    i32.const 0
    local.get $size
    memory.fill
    local.get $p
    local.get $poll
    i32.store offset={F_POLL}
    local.get $p
    local.get $kind
    i32.store offset={F_KIND}
    local.get $p
)
(func $dream_enqueue (param $f i32)
    local.get $f
    i32.eqz
    br_if 0
    local.get $f
    i32.load offset={F_QUEUED}
    br_if 0
    local.get $f
    i32.const 1
    i32.store offset={F_QUEUED}
    local.get $f
    i32.const 0
    i32.store offset={F_NEXT}
    global.get $rq_tail
    i32.eqz
    (if
        (then
            local.get $f
            global.set $rq_head
            local.get $f
            global.set $rq_tail
        )
        (else
            global.get $rq_tail
            local.get $f
            i32.store offset={F_NEXT}
            local.get $f
            global.set $rq_tail
        )
    )
)
(func $dream_complete (param $f i32) (param $res i32)
    (local $w i32)
    local.get $f
    i32.load offset={F_STATUS}
    br_if 0
    local.get $f
    local.get $res
    i32.store offset={F_RESULT}
    local.get $f
    i32.const 1
    i32.store offset={F_STATUS}
    local.get $f
    i32.load offset={F_WAKER}
    local.set $w
    local.get $w
    i32.eqz
    br_if 0
    local.get $f
    i32.const 0
    i32.store offset={F_WAKER}
    local.get $w
    local.get $f
    call $dream_wake
)
(func $dream_wake (param $w i32) (param $child i32)
    local.get $w
    i32.load offset={F_KIND}
    i32.eqz
    (if
        (then
            local.get $w
            call $dream_enqueue
        )
        (else
            local.get $w
            local.get $child
            call $dream_combinator_progress
        )
    )
)
(func $dream_await (param $parent i32) (param $child i32)
    local.get $child
    local.get $parent
    i32.store offset={F_WAKER}
    local.get $child
    i32.load offset={F_STATUS}
    (if
        (then
            local.get $parent
            call $dream_enqueue
        )
    )
)
(func $dream_resolve (param $f i32) (param $res i32)
    local.get $f
    local.get $res
    call $dream_complete
)
(func $dream_cancel (param $c i32)
    (local $cur i32)
    (local $nxt i32)
    local.get $c
    i32.eqz
    br_if 0
    local.get $c
    i32.load offset={F_STATUS}
    br_if 0
    local.get $c
    i32.const {STATUS_CANCELLED}
    i32.store offset={F_STATUS}
    local.get $c
    i32.const 0
    i32.store offset={F_WAKER}
    ;; unlink $c from the timer list so a pending timer never fires it
    global.get $timer_head
    local.get $c
    i32.eq
    (if
        (then
            local.get $c
            i32.load offset={F_NEXT}
            global.set $timer_head
        )
        (else
            global.get $timer_head
            local.set $cur
            (block $unlinked
                (loop $scan
                    local.get $cur
                    i32.eqz
                    br_if $unlinked
                    local.get $cur
                    i32.load offset={F_NEXT}
                    local.set $nxt
                    local.get $nxt
                    local.get $c
                    i32.eq
                    (if
                        (then
                            local.get $cur
                            local.get $c
                            i32.load offset={F_NEXT}
                            i32.store offset={F_NEXT}
                            br $unlinked
                        )
                    )
                    local.get $nxt
                    local.set $cur
                    br $scan
                )
            )
        )
    )
    local.get $c
    call $release_generic
)
(func $dream_set_timer (param $f i32) (param $delay i32)
    (local $due i32)
    (local $cur i32)
    (local $nxt i32)
    global.get $vclock
    local.get $delay
    i32.add
    local.set $due
    local.get $f
    local.get $due
    i32.store offset={F_DUE}
    global.get $timer_head
    i32.eqz
    (if
        (then
            local.get $f
            i32.const 0
            i32.store offset={F_NEXT}
            local.get $f
            global.set $timer_head
            return
        )
    )
    global.get $timer_head
    i32.load offset={F_DUE}
    local.get $due
    i32.gt_s
    (if
        (then
            local.get $f
            global.get $timer_head
            i32.store offset={F_NEXT}
            local.get $f
            global.set $timer_head
            return
        )
    )
    global.get $timer_head
    local.set $cur
    (block $done
        (loop $scan
            local.get $cur
            i32.load offset={F_NEXT}
            local.set $nxt
            local.get $nxt
            i32.eqz
            br_if $done
            local.get $nxt
            i32.load offset={F_DUE}
            local.get $due
            i32.gt_s
            br_if $done
            local.get $nxt
            local.set $cur
            br $scan
        )
    )
    local.get $f
    local.get $cur
    i32.load offset={F_NEXT}
    i32.store offset={F_NEXT}
    local.get $cur
    local.get $f
    i32.store offset={F_NEXT}
)
(func $dream_run_loop
    (local $f i32)
    (local $t i32)
    (block $alldone
        (loop $outer
            (block $drained
                (loop $drain
                    global.get $rq_head
                    local.set $f
                    local.get $f
                    i32.eqz
                    br_if $drained
                    local.get $f
                    i32.load offset={F_NEXT}
                    global.set $rq_head
                    global.get $rq_head
                    i32.eqz
                    (if
                        (then
                            i32.const 0
                            global.set $rq_tail
                        )
                    )
                    local.get $f
                    i32.const 0
                    i32.store offset={F_QUEUED}
                    local.get $f
                    i32.const 0
                    i32.store offset={F_NEXT}
                    ;; a cancelled/settled future may still sit in the ready queue; never poll it
                    local.get $f
                    i32.load offset={F_STATUS}
                    br_if $drain
                    local.get $f
                    local.get $f
                    i32.load offset={F_POLL}
                    call_indirect (type $dream_poll_t)
                    drop
                    br $drain
                )
            )
            global.get $timer_head
            i32.eqz
            br_if $alldone
            global.get $timer_head
            i32.load offset={F_DUE}
            global.set $vclock
            (block $timers_done
                (loop $tloop
                    global.get $timer_head
                    local.set $t
                    local.get $t
                    i32.eqz
                    br_if $timers_done
                    local.get $t
                    i32.load offset={F_DUE}
                    global.get $vclock
                    i32.gt_s
                    br_if $timers_done
                    local.get $t
                    i32.load offset={F_NEXT}
                    global.set $timer_head
                    local.get $t
                    i32.const 0
                    i32.store offset={F_NEXT}
                    local.get $t
                    i32.const 0
                    call $dream_complete
                    br $tloop
                )
            )
            br $outer
        )
    )
)
(func $dream_combinator_progress (param $w i32) (param $child i32)
    (local $n i32)
    (local $i i32)
    (local $arr i32)
    (local $c i32)
    local.get $w
    i32.load offset={F_KIND}
    i32.const {KIND_ALL}
    i32.eq
    (if
        (then
            local.get $w
            local.get $w
            i32.load offset={F_REMAINING}
            i32.const 1
            i32.sub
            i32.store offset={F_REMAINING}
            local.get $w
            i32.load offset={F_REMAINING}
            i32.eqz
            (if
                (then
                    local.get $w
                    i32.load offset={F_COUNT}
                    local.set $n
                    i32.const 4
                    local.get $n
                    i32.const 4
                    i32.mul
                    i32.add
                    i32.const {tag_array}
                    call $malloc
                    local.set $arr
                    local.get $arr
                    local.get $n
                    i32.store
                    i32.const 0
                    local.set $i
                    (block $fdone
                        (loop $f
                            local.get $i
                            local.get $n
                            i32.ge_s
                            br_if $fdone
                            local.get $w
                            i32.load offset={F_CHILDREN}
                            i32.const 4
                            i32.add
                            local.get $i
                            i32.const 4
                            i32.mul
                            i32.add
                            i32.load
                            local.set $c
                            local.get $arr
                            i32.const 4
                            i32.add
                            local.get $i
                            i32.const 4
                            i32.mul
                            i32.add
                            local.get $c
                            i32.load offset={F_RESULT}
                            i32.store
                            local.get $i
                            i32.const 1
                            i32.add
                            local.set $i
                            br $f
                        )
                    )
                    local.get $w
                    local.get $arr
                    i32.store offset={F_RESULTS}
                    ;; ownership of each result pointer has transferred into $arr; drop the
                    ;; parent's strong ref on every child frame now that its result is copied out
                    i32.const 0
                    local.set $i
                    (block $rdone
                        (loop $rloop
                            local.get $i
                            local.get $n
                            i32.ge_s
                            br_if $rdone
                            local.get $w
                            i32.load offset={F_CHILDREN}
                            i32.const 4
                            i32.add
                            local.get $i
                            i32.const 4
                            i32.mul
                            i32.add
                            i32.load
                            call $release_generic
                            local.get $i
                            i32.const 1
                            i32.add
                            local.set $i
                            br $rloop
                        )
                    )
                    local.get $w
                    local.get $arr
                    call $dream_complete
                )
            )
        )
        (else
            local.get $w
            i32.load offset={F_STATUS}
            i32.eqz
            (if
                (then
                    local.get $w
                    local.get $child
                    i32.load offset={F_RESULT}
                    call $dream_complete
                    ;; cancel every remaining loser, then drop the parent's strong ref on the winner
                    local.get $w
                    i32.load offset={F_COUNT}
                    local.set $n
                    local.get $w
                    i32.load offset={F_CHILDREN}
                    local.set $arr
                    i32.const 0
                    local.set $i
                    (block $cdone
                        (loop $cloop
                            local.get $i
                            local.get $n
                            i32.ge_s
                            br_if $cdone
                            local.get $arr
                            i32.const 4
                            i32.add
                            local.get $i
                            i32.const 4
                            i32.mul
                            i32.add
                            i32.load
                            local.set $c
                            local.get $c
                            local.get $child
                            i32.ne
                            (if
                                (then
                                    local.get $c
                                    call $dream_cancel
                                )
                            )
                            local.get $i
                            i32.const 1
                            i32.add
                            local.set $i
                            br $cloop
                        )
                    )
                    local.get $child
                    call $release_generic
                )
            )
        )
    )
)
(func $dream_all (param $arr i32) (result i32)
    (local $w i32)
    (local $n i32)
    (local $i i32)
    (local $c i32)
    local.get $arr
    i32.load
    local.set $n
    i32.const {F_SLOTS}
    i32.const -1
    i32.const {KIND_ALL}
    call $dream_new_future
    local.set $w
    local.get $w
    local.get $arr
    i32.store offset={F_CHILDREN}
    local.get $w
    local.get $n
    i32.store offset={F_COUNT}
    local.get $w
    local.get $n
    i32.store offset={F_REMAINING}
    local.get $n
    i32.eqz
    (if
        (then
            local.get $w
            local.get $arr
            call $dream_complete
            local.get $w
            return
        )
    )
    ;; take a strong ref on every child up front so the parent owns them all before any
    ;; synchronous completion can fire progress (and, for `any`, cancel the losers)
    i32.const 0
    local.set $i
    (block $retdone
        (loop $retloop
            local.get $i
            local.get $n
            i32.ge_s
            br_if $retdone
            local.get $arr
            i32.const 4
            i32.add
            local.get $i
            i32.const 4
            i32.mul
            i32.add
            i32.load
            call $retain
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $retloop
        )
    )
    i32.const 0
    local.set $i
    (block $done
        (loop $reg
            local.get $i
            local.get $n
            i32.ge_s
            br_if $done
            local.get $arr
            i32.const 4
            i32.add
            local.get $i
            i32.const 4
            i32.mul
            i32.add
            i32.load
            local.set $c
            local.get $c
            local.get $w
            i32.store offset={F_WAKER}
            local.get $c
            i32.load offset={F_STATUS}
            (if
                (then
                    local.get $w
                    local.get $c
                    call $dream_combinator_progress
                )
            )
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $reg
        )
    )
    local.get $w
)
(func $dream_any (param $arr i32) (result i32)
    (local $w i32)
    (local $n i32)
    (local $i i32)
    (local $c i32)
    local.get $arr
    i32.load
    local.set $n
    i32.const {F_SLOTS}
    i32.const -1
    i32.const {KIND_ANY}
    call $dream_new_future
    local.set $w
    local.get $w
    local.get $arr
    i32.store offset={F_CHILDREN}
    local.get $w
    local.get $n
    i32.store offset={F_COUNT}
    local.get $w
    local.get $n
    i32.store offset={F_REMAINING}
    ;; take a strong ref on every child up front so the parent owns them all before any
    ;; synchronous completion can fire progress (and, for `any`, cancel the losers)
    i32.const 0
    local.set $i
    (block $retdone
        (loop $retloop
            local.get $i
            local.get $n
            i32.ge_s
            br_if $retdone
            local.get $arr
            i32.const 4
            i32.add
            local.get $i
            i32.const 4
            i32.mul
            i32.add
            i32.load
            call $retain
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $retloop
        )
    )
    i32.const 0
    local.set $i
    (block $done
        (loop $reg
            local.get $i
            local.get $n
            i32.ge_s
            br_if $done
            local.get $arr
            i32.const 4
            i32.add
            local.get $i
            i32.const 4
            i32.mul
            i32.add
            i32.load
            local.set $c
            local.get $c
            local.get $w
            i32.store offset={F_WAKER}
            local.get $c
            i32.load offset={F_STATUS}
            (if
                (then
                    local.get $w
                    local.get $c
                    call $dream_combinator_progress
                )
            )
            local.get $i
            i32.const 1
            i32.add
            local.set $i
            br $reg
        )
    )
    local.get $w
)
