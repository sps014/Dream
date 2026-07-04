use super::*;
use crate::mir::async_emit::{slot_load, slot_store, AsyncSlots, F_AWAITING, F_RESULT, F_STATE};
use crate::mir::emit::valuetype::{ValueFrame, ValueLocalKind};
use std::collections::HashSet;

mod async_ops;
mod rvalue;

/// Emits one function as WAT (calls fall back to `$def{N}`, and field/index access has no layout, so
/// this is for layout-free unit tests; the pipeline uses [`emit_program`]/[`emit_module`]).
pub fn emit_function(func: &MirFunction, interner: &TypeInterner) -> String {
    emit_function_with(
        func,
        interner,
        &HashMap::new(),
        &HashMap::new(),
        &LayoutTable::default(),
        &IndexMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashSet::new(),
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_function_with(
    func: &MirFunction,
    interner: &TypeInterner,
    symbols: &HashMap<(DefId, Vec<TypeId>), String>,
    sigs: &HashMap<(DefId, Vec<TypeId>), Vec<TypeId>>,
    layouts: &LayoutTable,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
    func_table: &HashMap<(DefId, Vec<TypeId>), usize>,
    value_glue: &HashSet<TypeId>,
    debug: bool,
) -> String {
    let frame = ValueFrame::compute(func, interner);
    let mut e = Emitter {
        func,
        interner,
        symbols,
        sigs,
        layouts,
        strings,
        tags,
        func_table,
        value_glue,
        frame,
        out: String::new(),
        async_parent: None,
        async_user_locals: 0,
        debug,
    };
    e.emit();
    e.out
}

/// Emits the poll function of an async coroutine: a single state-machine dispatch over the full
/// lowered body (`func`), whose `Await` terminators suspend/resume. `slots` maps every frame-resident
/// local to its offset in the `Future` frame. See [`Emitter::emit_async_state_machine`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_async_poll(
    func: &MirFunction,
    interner: &TypeInterner,
    symbols: &HashMap<(DefId, Vec<TypeId>), String>,
    layouts: &LayoutTable,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
    ftable: &HashMap<(DefId, Vec<TypeId>), usize>,
    slots: &AsyncSlots,
    poll_sym: &str,
    user_local_count: usize,
    debug: bool,
) -> String {
    // Async bodies do not apply call-argument widening or value-struct shadow frames yet (both gated
    // elsewhere); empty maps disable those paths without extra plumbing through the transform.
    let sigs: HashMap<(DefId, Vec<TypeId>), Vec<TypeId>> = HashMap::new();
    let value_glue: HashSet<TypeId> = HashSet::new();
    let frame = ValueFrame::compute(func, interner);
    let mut e = Emitter {
        func,
        interner,
        symbols,
        sigs: &sigs,
        layouts,
        strings,
        tags,
        func_table: ftable,
        value_glue: &value_glue,
        frame,
        out: String::new(),
        // The poll body *is* the coroutine; completions release its own reference locals.
        async_parent: Some(func),
        async_user_locals: user_local_count,
        debug,
    };
    e.emit_async_state_machine(slots, poll_sym);
    e.out
}

struct Emitter<'a> {
    func: &'a MirFunction,
    interner: &'a TypeInterner,
    symbols: &'a HashMap<(DefId, Vec<TypeId>), String>,
    /// Callee `(def, instance)` → parameter types, for implicit widening of call arguments.
    sigs: &'a HashMap<(DefId, Vec<TypeId>), Vec<TypeId>>,
    layouts: &'a LayoutTable,
    strings: &'a IndexMap<String, u32>,
    tags: &'a HashMap<TypeId, i32>,
    func_table: &'a HashMap<(DefId, Vec<TypeId>), usize>,
    /// Value-struct types that require retain/drop glue (see [`valuetype`]).
    value_glue: &'a HashSet<TypeId>,
    /// Shadow-frame layout + ownership classification of this function's value-struct locals.
    frame: ValueFrame,
    out: String,
    /// When emitting inside an async poll segment, the enclosing task (for scope-exit release).
    async_parent: Option<&'a MirFunction>,
    /// In an async poll body, the count of persistent user locals (params + declared `let`s) at the
    /// front of `func.locals`; only these are released on completion. Synthetic lowering temporaries
    /// (await results, array/reassignment scratch) that follow are transient and not deep-released
    /// here (mirroring the pre-CFG async behavior), so no helper is needed for their element types.
    async_user_locals: usize,
    /// Generate `@name` annotations
    debug: bool,
}

impl Emitter<'_> {
    fn line(&mut self, s: &str) {
        let _ = writeln!(self.out, "{}", s);
    }

    /// The symbol for a call target: the resolved function symbol for `(def, instance args)` when
    /// known, else a `def{N}` fallback (runtime intrinsics and not-yet-emitted targets).
    fn callee_symbol(&self, callee: &crate::mir::Callee) -> String {
        self.symbols
            .get(&(callee.def, callee.args.clone()))
            .cloned()
            .unwrap_or_else(|| format!("def{}", callee.def.0))
    }

    fn emit(&mut self) {
        let mut params: String = self
            .func
            .params
            .iter()
            .map(|p| {
                let p_ty = self.wasm_ty(self.func.local_ty(*p));
                let name = &self.func.locals[p.0 as usize].name;
                if self.debug && name.is_some() {
                    format!(
                        " (param ${} (@name \"{}\") {})",
                        p.0,
                        name.as_ref().unwrap(),
                        p_ty
                    )
                } else {
                    format!(" (param ${} {})", p.0, p_ty)
                }
            })
            .collect();
        // A value(`struct`)-returning function uses the sret ABI: a hidden leading `$__sret` pointer
        // names the caller-provided destination the result is copied into, and the function itself
        // returns no WASM value.
        let result = if self.returns_value_struct() {
            params = format!(" (param $__sret i32){}", params);
            String::new()
        } else {
            match self.interner.kind(self.func.ret) {
                TyKind::Void => String::new(),
                _ => format!(" (result {})", self.wasm_ty(self.func.ret)),
            }
        };
        let sym = func_symbol(self.func);
        if self.debug {
            self.line(&format!(
                "(func ${} (@name \"{}\"){}{}",
                sym, self.func.name, params, result
            ));
        } else {
            self.line(&format!("(func ${}{}{}", sym, params, result));
        }

        // Non-parameter locals plus the dispatch program-counter.
        let param_count = self.func.params.len();
        for (i, decl) in self.func.locals.iter().enumerate() {
            if i < param_count {
                continue;
            }
            if let (true, Some(name)) = (self.debug, decl.name.as_ref()) {
                self.line(&format!(
                    "  (local ${} (@name \"{}\") {})",
                    i,
                    name,
                    self.wasm_ty(decl.ty)
                ));
            } else {
                self.line(&format!("  (local ${} {})", i, self.wasm_ty(decl.ty)));
            }
        }
        self.line("  (local $__pc i32)");
        if self.frame.size > 0 {
            // Saved shadow-stack pointer, restored before every return.
            self.line("  (local $__saved_sp i32)");
        }
        // Scratch pointer holding the object under construction across field initialization
        // (`New`/`ArrayLit`). Safe as a single slot: lowering materializes all args into operands,
        // so allocations never nest within a single rvalue.
        self.line("  (local $__obj i32)");
        // Scratch length for `Buffer.alloc<T>(len)`: the count is needed for both the allocation size
        // and the zero-fill, so it is materialized once here.
        self.line("  (local $__len i32)");
        // Scratch holding the previous occupant of a reference field/element across a reassignment, so
        // it can be released *after* the new value is stored (deferred release keeps a self-referential
        // `obj.f = g(obj.f)` sound).
        self.line("  (local $__rel i32)");
        // Scratch holding the saved `$__sp` across a dynamic `js` call's argument-slot buffer (see
        // `emit_js_call`): the buffer is bump-allocated below `$__sp` and released right after the
        // single host crossing, so this need only survive one rvalue.
        self.line("  (local $__jsp i32)");

        self.emit_value_frame_prologue();
        self.emit_dispatch();
        self.line(")");
    }

    /// Reserves this function's shadow-stack frame (for inline value(`struct`) locals): save `$__sp`,
    /// carve the frame by growing the stack downward, zero it (so drop-glue on a not-yet-assigned slot
    /// sees null reference fields), and point each owning value local at its slot.
    fn emit_value_frame_prologue(&mut self) {
        if self.frame.size == 0 {
            return;
        }
        let size = self.frame.size;
        self.line("     (global.get $__sp) (local.set $__saved_sp)");
        self.line(&format!(
            "     (global.get $__sp) (i32.const {}) (i32.sub) (global.set $__sp)",
            size
        ));
        // Zero the whole frame: memory.fill(dest = $__sp, value = 0, len = size).
        self.line(&format!(
            "     (global.get $__sp) (i32.const 0) (i32.const {}) (memory.fill)",
            size
        ));
        for (local, offset) in self.frame.owning_slots() {
            let ty = self.func.local_ty(local);
            let l0 = local.0;
            let slot_addr = |s: &mut Self| {
                s.line("     (global.get $__sp)");
                if offset > 0 {
                    s.line(&format!("     (i32.const {}) (i32.add)", offset));
                }
            };
            if self.frame.kind(local) == Some(ValueLocalKind::Param) {
                // A value param arrives as a pointer to the caller's value: copy those bytes into the
                // callee's private slot (retaining reference fields), then rebind the param to the slot
                // so the caller's value is never mutated (copy semantics).
                self.emit_value_copy(
                    slot_addr,
                    |s| s.line(&format!("     (local.get ${})", l0)),
                    ty,
                );
            }
            slot_addr(self);
            self.line(&format!("     (local.set ${})", l0));
        }
    }

    /// The labeled-block dispatch loop: each iteration reads `$__pc` and `br_table`s to the matching
    /// block; each block body ends by setting `$__pc` and branching back, or by returning.
    fn emit_dispatch(&mut self) {
        let n = self.func.blocks.len();
        self.line(&format!("  ;; entry = bb{}", self.func.entry.0));
        self.line(&format!("  (i32.const {})", self.func.entry.0));
        self.line("  (local.set $__pc)");
        self.line("  (block $__exit");
        self.line("   (loop $__loop");

        // Open one block per CFG block, innermost = bb0.
        for i in (0..n).rev() {
            self.line(&format!("    (block $bb{}", i));
        }
        // Dispatch from the innermost scope.
        let labels: String = (0..n).map(|i| format!("$bb{} ", i)).collect();
        let default = format!("$bb{}", n.saturating_sub(1));
        self.line(&format!(
            "     (br_table {}{} (local.get $__pc))",
            labels, default
        ));

        // After each `(block $bbK ...)` closes, that block's body runs.
        for i in 0..n {
            self.line(&format!("    ) ;; bb{} body", i));
            self.emit_block(crate::mir::BlockId(i as u32));
        }

        self.line("   )"); // loop
        self.line("  )"); // exit block
                          // Every block ends in a `return`/`goto`, so control never falls out of the dispatch loop.
                          // A value-returning function still needs its implicit `end` to be well-typed; mark the
                          // unreachable tail so the validator does not demand a phantom result value on the stack. A
                          // value-`struct` (sret) return produces no WASM result, so it needs no phantom either.
        if self.wasm_returns_value() {
            self.line("  (unreachable)");
        }
    }

    fn emit_block(&mut self, id: crate::mir::BlockId) {
        let block = self.func.block(id);
        for stmt in &block.stmts {
            self.emit_stmt(stmt);
        }
        self.emit_terminator(&block.terminator);
    }

    fn emit_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign(place, rvalue) => self.emit_assign(place, rvalue),
            Statement::Retain(o) => {
                self.emit_operand(o);
                self.line("     (call $retain)");
            }
            Statement::Release(o) => {
                // Deep release by the operand's declared type: structs/unions/reference arrays run
                // their generated `$release_<...>` (freeing fields + `del()`); other references fall
                // back to the generic/tag-dispatched runtime.
                let ty = self.operand_ty(o);
                let call = if self.interner.is_reference(ty) {
                    release_call(self.interner, self.layouts, ty)
                } else {
                    "$release_generic".to_string()
                };
                self.emit_operand(o);
                self.line(&format!("     (call {})", call));
            }
            Statement::Call { callee, args } => {
                self.emit_call_args(callee, args);
                self.line(&format!("     (call ${})", self.callee_symbol(callee)));
                if !matches!(self.interner.kind(callee.ret), TyKind::Void) {
                    self.line("     (drop)");
                }
            }
            Statement::InterfaceCall {
                receiver,
                iface_id,
                method_slot,
                sig,
                args,
            } => {
                self.emit_interface_call(receiver, *iface_id, *method_slot, *sig, args);
                let ret = match self.interner.kind(*sig) {
                    TyKind::Func(_, r) => Some(*r),
                    _ => None,
                };
                let drops = ret
                    .map(|r| !matches!(self.interner.kind(r), TyKind::Void))
                    .unwrap_or(false);
                if drops {
                    self.line("     (drop)");
                }
            }
            Statement::Print { arg, ty, newline } => {
                // Push the value, then print it. `int`/`char`/`string` go straight to a host import;
                // every other scalar is first rendered with its in-wasm `*_to_string` and printed as a
                // string. `println` appends a trailing newline (`\n` = 10) via `$print_char`.
                self.emit_operand(arg);
                match self.interner.kind(self.interner.strip_nullable(*ty)) {
                    TyKind::Prim(PrimTy::Int) => self.line("     (call $print_int)"),
                    TyKind::Prim(PrimTy::Char) => self.line("     (call $print_char)"),
                    TyKind::Prim(PrimTy::String) => self.line("     (call $print_string)"),
                    TyKind::Prim(prim) => {
                        let to_string = match prim {
                            PrimTy::Bool => "$bool_to_string",
                            PrimTy::Float => "$float_to_string",
                            PrimTy::Double => "$double_to_string",
                            PrimTy::Long => "$long_to_string",
                            PrimTy::UInt => "$uint_to_string",
                            PrimTy::ULong => "$ulong_to_string",
                            PrimTy::Byte => "$byte_to_string",
                            // Int/Char/String handled above; any other primitive prints via $print_int.
                            _ => "",
                        };
                        if to_string.is_empty() {
                            self.line("     (call $print_int)");
                        } else {
                            self.line(&format!("     (call {})", to_string));
                            self.line("     (call $print_string)");
                        }
                    }
                    // Enums are `i32` values at runtime; print their numeric value.
                    TyKind::Enum(_) => self.line("     (call $print_int)"),
                    // Arrays aren't self-describing at runtime (the header only says `TAG_ARRAY`), so
                    // the element-typed `to_string` is chosen statically here, then printed.
                    TyKind::Array(elem) => {
                        self.line(&format!("     (call {})", array_to_string_sym(*elem)));
                        self.line("     (call $print_string)");
                    }
                    // A value struct/union has no heap tag header, so it is rendered by its concrete
                    // `$<Type>_to_string` (chosen statically from the operand's type) and printed.
                    _ if self.interner.is_value_type(*ty) => {
                        if let Some(name) = self.value_name(*ty) {
                            self.line(&format!("     (call ${}_to_string)", name));
                            self.line("     (call $print_string)");
                        } else {
                            self.line("     (call $print_object)");
                        }
                    }
                    // Reference structs, unions, and `object` render through the tag-dispatching
                    // `$print_object` (which routes to each type's `to_string`).
                    _ => self.line("     (call $print_object)"),
                }
                if *newline {
                    self.line("     (i32.const 10)");
                    self.line("     (call $print_char)");
                }
            }
            Statement::Nop => {}
        }
    }

    fn emit_assign(&mut self, place: &Place, rvalue: &Rvalue) {
        match place {
            Place::Local(l) => {
                let ty = self.func.local_ty(*l);
                if self.interner.is_value_type(ty) {
                    let l0 = l.0;
                    match self.frame.kind(*l) {
                        Some(ValueLocalKind::Owning) => {
                            self.emit_value_store(
                                |s| s.line(&format!("     (local.get ${})", l0)),
                                ty,
                                rvalue,
                            );
                        }
                        // A borrow/param value local just holds an address: rebind it to the source
                        // value's address (no copy, no drop).
                        _ => {
                            match rvalue {
                                Rvalue::Use(o) => self.emit_operand_addr(o),
                                _ => self.emit_rvalue(rvalue),
                            }
                            self.line(&format!("     (local.set ${})", l0));
                        }
                    }
                    return;
                }
                self.emit_rvalue(rvalue);
                self.line(&format!("     (local.set ${})", l.0));
            }
            Place::Global(g) => {
                self.emit_rvalue(rvalue);
                self.line(&format!("     (global.set $g{})", g.0));
            }
            Place::Field { base, field } => {
                if let Some((off, fty)) = self.field_layout(*base, *field) {
                    let (b, off, fty) = (*base, off, fty);
                    if self.interner.is_value_type(fty) {
                        self.emit_value_store(move |s| s.field_addr(b, off), fty, rvalue);
                        return;
                    }
                    let stash = self.stash_old_ref(fty, |s| s.field_addr(b, off));
                    self.field_addr(*base, off);
                    self.emit_rvalue(rvalue);
                    self.line(&format!("     ({})", self.store_instr(fty)));
                    self.retain_stored_rvalue(fty, rvalue);
                    self.release_stash(fty, stash);
                } else {
                    unreachable!("Missing field layout for store");
                }
            }
            Place::Index { base, index } => {
                if let Some(ety) = self.array_elem_ty(*base) {
                    let (b, idx) = (*base, index.clone());
                    if self.interner.is_value_type(ety) {
                        let idx2 = idx.clone();
                        self.emit_value_store(move |s| s.elem_addr(b, ety, &idx2), ety, rvalue);
                        return;
                    }
                    let stash = self.stash_old_ref(ety, |s| s.elem_addr(b, ety, &idx));
                    self.elem_addr(*base, ety, index);
                    self.emit_rvalue(rvalue);
                    self.line(&format!("     ({})", self.store_instr(ety)));
                    self.retain_stored_rvalue(ety, rvalue);
                    self.release_stash(ety, stash);
                } else {
                    unreachable!("Missing array element type for store");
                }
            }
        }
    }

    /// Pushes the address of `base.field` (`base + offset`) onto the stack.
    /// The runtime tag to stamp into a newly allocated value of `ty`: its assigned struct/union tag,
    /// or the `DefId` as a last-resort fallback (only when no layout/tag is registered).
    fn type_tag(&self, ty: TypeId, fallback: DefId) -> i32 {
        self.tags.get(&ty).copied().unwrap_or(fallback.0 as i32)
    }

    /// The heap address of an interned string. Every string literal reachable in codegen is
    /// harvested into the interner beforehand (see `strings_in_*`), so a miss is a harvesting bug,
    /// not a user error — fail loudly instead of emitting a null (address 0) string.
    fn string_addr(&self, s: &str) -> u32 {
        self.strings.get(s).copied().unwrap_or_else(|| {
            unreachable!("string literal {:?} was not interned before codegen", s)
        })
    }

    fn field_addr(&mut self, base: crate::mir::Local, offset: u32) {
        self.line(&format!("     (local.get ${})", base.0));
        if offset > 0 {
            self.line(&format!("     (i32.const {})", offset));
            self.line("     (i32.add)");
        }
    }

    /// Stores `value` into the object under construction (`$__obj + offset`) with the field/element
    /// width. Used by `New`/`ArrayLit` initialization. A *borrowed* reference (a copy of an existing
    /// place) is retained, since the container becomes a new owner; an owned producer is not
    /// materialized here (lowering routes those through a temporary that is itself released at scope
    /// exit), so retaining a copied operand is the sound, uniform rule.
    fn store_at_obj(&mut self, offset: u32, value_ty: TypeId, value: &Operand) {
        // A value struct stored into a freshly-allocated container is copied inline (byte-wise + a
        // retain of its reference fields); the block was just zeroed, so there is no old value to
        // drop.
        if self.interner.is_value_type(value_ty) {
            let value = value.clone();
            self.emit_value_copy(
                |s| {
                    s.line("     (local.get $__obj)");
                    if offset > 0 {
                        s.line(&format!("     (i32.const {}) (i32.add)", offset));
                    }
                },
                |s| s.emit_operand_addr(&value),
                value_ty,
            );
            return;
        }
        self.line("     (local.get $__obj)");
        if offset > 0 {
            self.line(&format!("     (i32.const {})", offset));
            self.line("     (i32.add)");
        }
        self.emit_operand(value);
        self.line(&format!("     ({})", self.store_instr(value_ty)));
        self.retain_container_value(value_ty, value);
    }

    /// Emits a `$retain` of a reference value being stored into a container (struct field, array
    /// element, or union payload), so the container owns its own reference count. A no-op for
    /// non-reference values and for non-place operands (constants/null; `$retain` also null-guards).
    fn retain_container_value(&mut self, value_ty: TypeId, value: &Operand) {
        let borrowed = matches!(value, Operand::Copy(_) | Operand::Const(Const::Str(_)));
        if self.interner.is_reference(value_ty) && borrowed {
            self.emit_operand(value);
            self.line("     (call $retain)");
        }
    }

    /// Before a reference field/element is overwritten, load and stash its previous occupant into the
    /// `$__rel` scratch so it can be released *after* the new value is stored (a deferred release keeps
    /// self-referential reassignments like `n.next = f(n.next)` sound). `emit_addr` pushes the slot's
    /// address. Returns `true` when a value was stashed (the slot is a reference). A no-op for
    /// non-reference slots, and releasing a null previous value (fresh field) is a runtime no-op.
    fn stash_old_ref(&mut self, ty: TypeId, emit_addr: impl Fn(&mut Self)) -> bool {
        if !self.interner.is_reference(ty) {
            return false;
        }
        emit_addr(self);
        self.line("     (i32.load)");
        self.line("     (local.set $__rel)");
        true
    }

    /// Releases the value stashed by [`Self::stash_old_ref`] (the overwritten field/element's previous
    /// occupant), if any.
    fn release_stash(&mut self, ty: TypeId, stashed: bool) {
        if !stashed {
            return;
        }
        let call = release_call(self.interner, self.layouts, ty);
        self.line("     (local.get $__rel)");
        self.line(&format!("     (call {})", call));
    }

    /// Like [`Self::retain_container_value`] but for a field/element written from an rvalue: a
    /// *borrowed* value (`Use(Copy(place))`) is retained, while an owned producer (call/new/array
    /// literal result) transfers its `+1` into the container and is left as-is.
    fn retain_stored_rvalue(&mut self, ty: TypeId, rvalue: &Rvalue) {
        if let Rvalue::Use(value) = rvalue {
            self.retain_container_value(ty, value);
        }
    }

    /// Writes a zero of `field_ty`'s width into the object under construction (`$__obj + offset`).
    /// Used to clear a struct before a user constructor runs (reused heap blocks are not zeroed).
    fn zero_at_obj(&mut self, offset: u32, field_ty: TypeId) {
        self.line("     (local.get $__obj)");
        if offset > 0 {
            self.line(&format!("     (i32.const {})", offset));
            self.line("     (i32.add)");
        }
        let zero = match self.store_instr(field_ty) {
            "f64.store" => "(f64.const 0)",
            "f32.store" => "(f32.const 0)",
            "i64.store" => "(i64.const 0)",
            _ => "(i32.const 0)",
        };
        self.line(&format!("     {}", zero));
        self.line(&format!("     ({})", self.store_instr(field_ty)));
    }

    /// Pushes the address of `base[index]` (`base + 4 + index * elem_size`) onto the stack. The
    /// length occupies the first word, so element 0 is at offset 4.
    fn elem_addr(&mut self, base: crate::mir::Local, elem_ty: TypeId, index: &Operand) {
        let (size, _) = scalar_size(self.interner, elem_ty);
        self.line(&format!("     (local.get ${})", base.0));
        self.line("     (i32.const 4)");
        self.line("     (i32.add)");
        self.emit_operand(index);
        self.line(&format!("     (i32.const {})", size));
        self.line("     (i32.mul)");
        self.line("     (i32.add)");
    }

    /// The struct field's `(byte offset, type)` from the layout table, or `None` when `base` is not a
    /// laid-out nominal type (e.g. a union, or a type whose layout was not recorded).
    fn field_layout(&self, base: crate::mir::Local, field: usize) -> Option<(u32, TypeId)> {
        let bty = self.interner.strip_nullable(self.func.local_ty(base));
        // Layouts are keyed by the full (monomorphized) type id, so `Box<int>` and `Box<string>`
        // resolve to their own field widths.
        let f = self.layouts.get(bty)?.fields.get(field)?;
        Some((f.offset, f.ty))
    }

    /// The element type of an array-typed local, or `None` if `base` is not an array.
    fn array_elem_ty(&self, base: crate::mir::Local) -> Option<TypeId> {
        match self
            .interner
            .kind(self.interner.strip_nullable(self.func.local_ty(base)))
        {
            TyKind::Array(e) => Some(*e),
            _ => None,
        }
    }

    /// The load instruction for a value of `ty` (width- and float-aware; sub-word loads are unsigned).
    fn load_instr(&self, ty: TypeId) -> &'static str {
        match self.interner.kind(self.interner.strip_nullable(ty)) {
            TyKind::Prim(PrimTy::Float) => "f32.load",
            TyKind::Prim(PrimTy::Double) => "f64.load",
            TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64.load",
            TyKind::Prim(PrimTy::Bool | PrimTy::Char | PrimTy::Byte) => "i32.load8_u",
            _ => "i32.load",
        }
    }

    /// The store instruction matching [`Self::load_instr`].
    fn store_instr(&self, ty: TypeId) -> &'static str {
        match self.interner.kind(self.interner.strip_nullable(ty)) {
            TyKind::Prim(PrimTy::Float) => "f32.store",
            TyKind::Prim(PrimTy::Double) => "f64.store",
            TyKind::Prim(PrimTy::Long | PrimTy::ULong) => "i64.store",
            TyKind::Prim(PrimTy::Bool | PrimTy::Char | PrimTy::Byte) => "i32.store8",
            _ => "i32.store",
        }
    }

    // ---- Value (`struct`) type helpers -------------------------------------------------------

    /// True when this function returns a value struct by the sret ABI (a hidden `$__sret` pointer)
    /// rather than as a WASM result.
    fn returns_value_struct(&self) -> bool {
        self.interner.is_value_type(self.func.ret)
    }

    /// True when this function returns an ordinary WASM value (non-void, non-value-struct).
    fn wasm_returns_value(&self) -> bool {
        !matches!(self.interner.kind(self.func.ret), TyKind::Void) && !self.returns_value_struct()
    }

    /// The inline byte size of value struct `ty`.
    fn value_size(&self, ty: TypeId) -> u32 {
        scalar_size(self.interner, ty).0
    }

    /// True when value struct `ty` needs retain/drop glue (embeds references or declares `del`).
    fn value_has_glue(&self, ty: TypeId) -> bool {
        self.value_glue.contains(&self.interner.strip_nullable(ty))
    }

    /// The layout name of value type `ty` (a value struct or value union), if laid out. Used to name
    /// its retain/drop glue.
    fn value_name(&self, ty: TypeId) -> Option<String> {
        let stripped = self.interner.strip_nullable(ty);
        if let Some(l) = self.layouts.get(stripped) {
            return Some(l.name.clone());
        }
        self.layouts.union(stripped).map(|u| u.name.clone())
    }

    /// Pushes the address of value place `p` (a value struct is addressed, never loaded).
    fn emit_place_addr(&mut self, p: &Place) {
        match p {
            Place::Local(l) => self.line(&format!("     (local.get ${})", l.0)),
            Place::Field { base, field } => {
                if let Some((off, _)) = self.field_layout(*base, *field) {
                    self.field_addr(*base, off);
                }
            }
            Place::Index { base, index } => {
                if let Some(ety) = self.array_elem_ty(*base) {
                    self.elem_addr(*base, ety, index);
                }
            }
            Place::Global(_) => self.line("     (i32.const 0) ;; value-struct global unsupported"),
        }
    }

    /// Pushes the address of a value-struct operand.
    fn emit_operand_addr(&mut self, o: &Operand) {
        match o {
            Operand::Copy(p) => self.emit_place_addr(p),
            Operand::Const(_) => self.line("     (i32.const 0)"),
        }
    }

    /// Byte-wise copies value struct `ty` from the `src` address to the `dst` address, then retains
    /// the destination's (now duplicated) reference fields so the copy owns its own references.
    fn emit_value_copy(&mut self, dst: impl Fn(&mut Self), src: impl Fn(&mut Self), ty: TypeId) {
        let size = self.value_size(ty);
        dst(self);
        src(self);
        self.line(&format!("     (i32.const {})", size));
        self.line("     (memory.copy)");
        if self.value_has_glue(ty) {
            if let Some(name) = self.value_name(ty) {
                dst(self);
                self.line(&format!("     (call {})", vs_retain_sym(&name)));
            }
        }
    }

    /// Drops the value struct `ty` at the `at` address (runs `del`, releases reference fields), if it
    /// needs glue.
    fn emit_value_drop(&mut self, at: impl Fn(&mut Self), ty: TypeId) {
        if self.value_has_glue(ty) {
            if let Some(name) = self.value_name(ty) {
                at(self);
                self.line(&format!("     (call {})", vs_drop_sym(&name)));
            }
        }
    }

    /// Constructs a value struct in place at the `dst` address: zero its bytes, then (if it has a
    /// user constructor) call `ctor(this = dst, args...)`.
    fn construct_value_new(
        &mut self,
        dst: impl Fn(&mut Self),
        ctor: Option<DefId>,
        args: &[Operand],
        ty: TypeId,
    ) {
        let size = self.value_size(ty);
        dst(self);
        self.line("     (i32.const 0)");
        self.line(&format!("     (i32.const {})", size));
        self.line("     (memory.fill)");
        if let Some(ctor) = ctor {
            dst(self);
            for arg in args {
                self.emit_operand(arg);
            }
            let sym = self.callee_symbol(&crate::mir::Callee {
                def: ctor,
                args: vec![],
                ret: self.interner.void(),
            });
            self.line(&format!("     (call ${})", sym));
        }
    }

    /// Emits a direct call to a value-struct-returning function using the sret ABI: the destination
    /// address (produced by `dst`) is passed as the hidden leading argument, then the real arguments.
    fn emit_value_sret_call(
        &mut self,
        dst: impl Fn(&mut Self),
        callee: &crate::mir::Callee,
        args: &[Operand],
    ) {
        dst(self);
        self.emit_call_args(callee, args);
        self.line(&format!("     (call ${})", self.callee_symbol(callee)));
    }

    /// Constructs a value union in place at the `dst` address: zero the block, write the variant
    /// discriminant at offset 0, then store each payload argument at its variant field offset (a
    /// value payload is copied inline; a reference payload is stored and retained).
    fn construct_value_union(
        &mut self,
        dst: impl Fn(&mut Self),
        ty: TypeId,
        variant: usize,
        args: &[Operand],
    ) {
        let size = self.value_size(ty);
        dst(self);
        self.line("     (i32.const 0)");
        self.line(&format!("     (i32.const {})", size));
        self.line("     (memory.fill)");
        dst(self);
        self.line(&format!("     (i32.const {}) ;; discriminant", variant));
        self.line("     (i32.store)");
        let fields: Vec<(u32, TypeId)> = self
            .layouts
            .union(ty)
            .and_then(|u| {
                u.variants
                    .iter()
                    .find(|v| v.discriminant as usize == variant)
                    .map(|v| v.fields.iter().map(|f| (f.offset, f.ty)).collect())
            })
            .unwrap_or_default();
        for (i, arg) in args.iter().enumerate() {
            let Some(&(off, fty)) = fields.get(i) else {
                continue;
            };
            let field_addr = |s: &mut Self| {
                dst(s);
                if off > 0 {
                    s.line(&format!("     (i32.const {}) (i32.add)", off));
                }
            };
            if self.interner.is_value_type(fty) {
                let arg = arg.clone();
                self.emit_value_copy(field_addr, |s| s.emit_operand_addr(&arg), fty);
            } else {
                field_addr(self);
                self.emit_operand(arg);
                self.line(&format!("     ({})", self.store_instr(fty)));
                self.retain_container_value(fty, arg);
            }
        }
    }

    /// Stores a value struct or value union produced by `rvalue` into the destination at the `dst`
    /// address (a local slot, a container field/element, or a union payload): the old contents are
    /// dropped, then the new value is constructed / sret-called / copied in place.
    fn emit_value_store(&mut self, dst: impl Fn(&mut Self), ty: TypeId, rvalue: &Rvalue) {
        self.emit_value_drop(&dst, ty);
        match rvalue {
            Rvalue::New {
                ctor,
                args,
                ty: nty,
                ..
            } => self.construct_value_new(&dst, *ctor, args, *nty),
            Rvalue::UnionNew {
                ty: uty,
                variant,
                args,
                ..
            } => self.construct_value_union(&dst, *uty, *variant, args),
            Rvalue::Call { callee, args } => self.emit_value_sret_call(&dst, callee, args),
            Rvalue::InterfaceCall {
                receiver,
                iface_id,
                method_slot,
                sig,
                args,
                ..
            } => self.emit_interface_sret_call(&dst, receiver, *iface_id, *method_slot, *sig, args),
            Rvalue::Use(Operand::Copy(src)) => {
                let src = src.clone();
                self.emit_value_copy(&dst, |s| s.emit_place_addr(&src), ty);
            }
            other => {
                // Any other value-struct-producing rvalue (e.g. a `UnionField` payload extraction)
                // yields the *address* of an existing value; copy those bytes into the destination.
                let other = other.clone();
                self.emit_value_copy(&dst, |s| s.emit_rvalue(&other), ty);
            }
        }
    }

    /// Emits the scope-exit teardown of a function's shadow frame: drop each owning value local, then
    /// restore `$__sp`. A no-op for functions with no value frame.
    fn emit_frame_teardown(&mut self) {
        for (local, _) in self.frame.owning_slots() {
            let ty = self.func.local_ty(local);
            let l0 = local.0;
            self.emit_value_drop(|s| s.line(&format!("     (local.get ${})", l0)), ty);
        }
        if self.frame.size > 0 {
            self.line("     (local.get $__saved_sp) (global.set $__sp)");
        }
    }

    /// Emits a dynamic `js` call marshaling its arguments through the shadow stack in one host
    /// crossing (no per-argument boxing, no heap array): save `$__sp`, carve `argc * 16` bytes,
    /// write one 16-byte tagged slot per argument (`[tag][aux][payload]`), call the bridge with
    /// `(target, [namePtr,] argsPtr, argc)`, and restore `$__sp` (the `i32` result handle stays on
    /// the WASM stack). The buffer lives below the value-struct frame and is released immediately, so
    /// it is allocation-free and re-entrant (a nested `js` call saves/restores its own `$__sp`).
    fn emit_js_call(
        &mut self,
        callee: &crate::mir::Callee,
        target: &Operand,
        method: Option<&Operand>,
        args: &[(Operand, TypeId)],
    ) {
        use crate::mir::js_abi;
        let argc = args.len() as u32;
        // Save `$__sp` and carve the slot buffer (skipped for a zero-argument call).
        self.line("     (global.get $__sp) (local.set $__jsp)");
        if argc > 0 {
            self.line(&format!(
                "     (global.get $__sp) (i32.const {}) (i32.sub) (global.set $__sp)",
                argc * js_abi::SLOT_SIZE
            ));
        }
        for (i, (op, ty)) in args.iter().enumerate() {
            let base = (i as u32) * js_abi::SLOT_SIZE;
            let (tag, aux, store) = js_abi::slot_desc(self.interner, *ty);
            self.emit_slot_word(base, &format!("(i32.const {})", tag));
            self.emit_slot_word(
                base + js_abi::SLOT_AUX_OFFSET,
                &format!("(i32.const {})", aux),
            );
            // Payload: the argument value, stored at its natural width.
            self.line(&format!(
                "     (global.get $__sp) (i32.const {}) (i32.add)",
                base + js_abi::SLOT_PAYLOAD_OFFSET
            ));
            self.emit_operand(op);
            self.line(&format!("     ({})", store));
        }
        // Bridge args: target, [namePtr,] argsPtr (= current $__sp), argc.
        self.emit_operand(target);
        if let Some(name) = method {
            self.emit_operand(name);
        }
        self.line("     (global.get $__sp)");
        self.line(&format!("     (i32.const {})", argc));
        let sym = self.callee_symbol(callee);
        self.line(&format!("     (call ${sym})"));
        // Release the buffer; the call's `i32` result remains beneath on the WASM stack.
        self.line("     (local.get $__jsp) (global.set $__sp)");
    }

    /// Stores an `i32` `value` (a WAT snippet leaving one `i32` on the stack) into the argument-slot
    /// buffer at byte offset `off` from `$__sp` — used by [`emit_js_call`](Self::emit_js_call) for a
    /// slot's `tag`/`aux` header words.
    fn emit_slot_word(&mut self, off: u32, value: &str) {
        self.line(&format!(
            "     (global.get $__sp) (i32.const {}) (i32.add)",
            off
        ));
        self.line(&format!("     {} (i32.store)", value));
    }

    fn emit_terminator(&mut self, t: &Terminator) {
        match t {
            Terminator::Goto(b) => self.goto(*b),
            Terminator::If {
                cond,
                then_blk,
                else_blk,
            } => {
                self.emit_operand(cond);
                self.line("     (if (then");
                self.goto(*then_blk);
                self.line("     ) (else");
                self.goto(*else_blk);
                self.line("     ))");
            }
            Terminator::Switch {
                value,
                targets,
                default,
            } => {
                // Lower to a chain of compares; a real br_table needs contiguous keys.
                for (k, b) in targets {
                    self.emit_operand(value);
                    self.line(&format!("     (i32.const {})", k));
                    self.line("     (i32.eq)");
                    self.line("     (if (then");
                    self.goto(*b);
                    self.line("     ))");
                }
                self.goto(*default);
            }
            Terminator::Return(Some(o)) => {
                if self.returns_value_struct() {
                    // sret ABI: copy the result into the caller-provided `$__sret` slot (retaining
                    // its reference fields) before the frame teardown drops the source local.
                    let o = o.clone();
                    let ty = self.func.ret;
                    self.emit_value_copy(
                        |s| s.line("     (local.get $__sret)"),
                        |s| s.emit_operand_addr(&o),
                        ty,
                    );
                    self.emit_frame_teardown();
                    self.line("     (return)");
                } else {
                    self.emit_operand(o);
                    self.emit_frame_teardown();
                    self.line("     (return)");
                }
            }
            Terminator::Return(None) => {
                self.emit_frame_teardown();
                self.line("     (return)");
            }
            Terminator::TailCall { callee, args } => {
                let sym = self.callee_symbol(callee);
                if let Some(kind) = async_intrinsic_kind(&sym) {
                    // Async intrinsics have a bespoke calling convention and can't be tail-called;
                    // fall back to `f(args); return`. (The `tco` pass avoids this, so it is only a
                    // safety net.)
                    self.emit_async_intrinsic(kind, args);
                    if !self.wasm_returns_value() {
                        self.line("     (drop)");
                    }
                    self.emit_frame_teardown();
                    self.line("     (return)");
                } else {
                    // Arguments are all scalar (the pass guarantees no value-struct/sret ABI), so
                    // the frame teardown below never touches them: it only drops this frame's inline
                    // value-struct slots and restores `$__sp`, leaving the pushed args intact for
                    // `return_call` to consume.
                    self.emit_call_args(callee, args);
                    self.emit_frame_teardown();
                    self.line(&format!("     (return_call ${})", sym));
                }
            }
            Terminator::Unreachable => self.line("     (unreachable)"),
            Terminator::AsyncComplete(_) => self.line("     (unreachable) ;; async in sync fn"),
            Terminator::Await { .. } => self.line("     (unreachable) ;; await in sync fn"),
        }
    }

    /// A CFG edge: set the dispatch PC to the target and loop back to re-dispatch.
    fn goto(&mut self, target: crate::mir::BlockId) {
        self.line(&format!("     (i32.const {})", target.0));
        self.line("     (local.set $__pc)");
        self.line("     (br $__loop)");
    }

    fn emit_operand(&mut self, op: &Operand) {
        match op {
            Operand::Const(c) => self.emit_const(c),
            Operand::Copy(Place::Local(l)) => self.line(&format!("     (local.get ${})", l.0)),
            Operand::Copy(Place::Global(g)) => self.line(&format!("     (global.get $g{})", g.0)),
            Operand::Copy(Place::Field { base, field }) => {
                if let Some((off, fty)) = self.field_layout(*base, *field) {
                    self.field_addr(*base, off);
                    // A value-struct field is addressed inline, not loaded: reading it yields the
                    // address of its inline storage (the consumer copies where a value is needed).
                    if !self.interner.is_value_type(fty) {
                        self.line(&format!("     ({})", self.load_instr(fty)));
                    }
                } else {
                    unreachable!("Missing field layout for read");
                }
            }
            Operand::Copy(Place::Index { base, index }) => {
                if let Some(ety) = self.array_elem_ty(*base) {
                    self.elem_addr(*base, ety, index);
                    if !self.interner.is_value_type(ety) {
                        self.line(&format!("     ({})", self.load_instr(ety)));
                    }
                } else {
                    unreachable!("Missing array element type for read");
                }
            }
        }
    }

    fn emit_const(&mut self, c: &Const) {
        match c {
            Const::Int(v) => self.line(&format!("     (i32.const {})", v)),
            Const::Long(v) => self.line(&format!("     (i64.const {})", v)),
            Const::Float(v) => self.line(&format!("     (f64.const {})", v)),
            Const::F32(v) => self.line(&format!("     (f32.const {})", v)),
            Const::Bool(v) => self.line(&format!("     (i32.const {})", *v as i32)),
            Const::Char(v) => self.line(&format!("     (i32.const {})", *v as u32)),
            Const::Null => self.line("     (i32.const 0)"),
            Const::Str(s) => match self.strings.get(s) {
                Some(addr) => self.line(&format!("     (i32.const {})", addr)),
                None => unreachable!("Missing interned string: {}", s),
            },
        }
    }

    fn operand_ty(&self, op: &Operand) -> TypeId {
        match op {
            Operand::Copy(Place::Local(l)) => self.func.local_ty(*l),
            Operand::Copy(Place::Field { base, field }) => self
                .field_layout(*base, *field)
                .map(|(_, t)| t)
                .unwrap_or_else(|| self.func.local_ty(*base)),
            Operand::Copy(Place::Index { base, .. }) => self
                .array_elem_ty(*base)
                .unwrap_or_else(|| self.func.local_ty(*base)),
            Operand::Copy(Place::Global(_)) => self.interner.int(),
            Operand::Const(Const::Long(_)) => self.interner.long(),
            Operand::Const(Const::Float(_)) => self.interner.double(),
            Operand::Const(Const::F32(_)) => self.interner.float(),
            // A char/bool/string constant keeps its own primitive type so type-directed dispatch
            // (e.g. `to_string`/`hash_code`, boxing into `object`) picks the right helper rather than
            // defaulting to `int`.
            Operand::Const(Const::Char(_)) => self.interner.char(),
            Operand::Const(Const::Bool(_)) => self.interner.bool(),
            Operand::Const(Const::Str(_)) => self.interner.string(),
            Operand::Const(_) => self.interner.int(),
        }
    }

    fn wasm_ty(&self, ty: TypeId) -> String {
        match self.interner.kind(self.interner.strip_nullable(ty)) {
            TyKind::Prim(PrimTy::Double | PrimTy::Long | PrimTy::ULong) => {
                match self.interner.kind(self.interner.strip_nullable(ty)) {
                    TyKind::Prim(PrimTy::Double) => "f64".to_string(),
                    _ => "i64".to_string(),
                }
            }
            TyKind::Prim(PrimTy::Float) => "f32".to_string(),
            TyKind::Void => "i32".to_string(),
            _ => "i32".to_string(),
        }
    }

    fn binop_instr(&self, op: BinOp, ty: TypeId) -> String {
        let w = self.wasm_ty(ty);
        let signed = !matches!(
            self.interner.kind(self.interner.strip_nullable(ty)),
            TyKind::Prim(PrimTy::UInt | PrimTy::ULong | PrimTy::Byte)
        );
        let s = if signed { "_s" } else { "_u" };
        let is_float = w == "f32" || w == "f64";
        match op {
            BinOp::Add => format!("{}.add", w),
            BinOp::Sub => format!("{}.sub", w),
            BinOp::Mul => format!("{}.mul", w),
            BinOp::Div if is_float => format!("{}.div", w),
            BinOp::Div => format!("{}.div{}", w, s),
            BinOp::Rem => format!("{}.rem{}", w, s),
            BinOp::Eq => format!("{}.eq", w),
            BinOp::Ne => format!("{}.ne", w),
            BinOp::Lt if is_float => format!("{}.lt", w),
            BinOp::Lt => format!("{}.lt{}", w, s),
            BinOp::Le if is_float => format!("{}.le", w),
            BinOp::Le => format!("{}.le{}", w, s),
            BinOp::Gt if is_float => format!("{}.gt", w),
            BinOp::Gt => format!("{}.gt{}", w, s),
            BinOp::Ge if is_float => format!("{}.ge", w),
            BinOp::Ge => format!("{}.ge{}", w, s),
            BinOp::And | BinOp::BitAnd => format!("{}.and", w),
            BinOp::Or | BinOp::BitOr => format!("{}.or", w),
            BinOp::BitXor => format!("{}.xor", w),
            BinOp::Shl => format!("{}.shl", w),
            BinOp::Shr => format!("{}.shr{}", w, s),
        }
    }
}
