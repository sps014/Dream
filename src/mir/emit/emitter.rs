use super::*;
use crate::mir::emit::valuetype::{ValueFrame, ValueLocalKind};
use std::collections::HashSet;

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
    };
    e.emit();
    e.out
}

/// Emits one basic block's straight-line body (no CFG dispatch loop). Used by async poll segments.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_straight_line_segment(
    func: &MirFunction,
    interner: &TypeInterner,
    symbols: &HashMap<(DefId, Vec<TypeId>), String>,
    layouts: &LayoutTable,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
    ftable: &HashMap<(DefId, Vec<TypeId>), usize>,
    async_parent: &MirFunction,
) -> String {
    // Async poll segments do not apply call-argument widening yet (async cases are still gated); an
    // empty signature map disables it without extra plumbing through the coroutine transform.
    let sigs: HashMap<(DefId, Vec<TypeId>), Vec<TypeId>> = HashMap::new();
    // Async poll segments do not participate in value-struct shadow frames yet (value structs in
    // async bodies are gated); an empty frame/glue set disables the value-type paths.
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
        async_parent: Some(async_parent),
    };
    e.emit_poll_segment_body();
    e.out
}

/// Evaluates a HIR expression and stores the result in `$__scratch` (async poll suspend).
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_expr_to_scratch(
    hir: &crate::hir::HFunction,
    expr: &crate::hir::HExpr,
    interner: &TypeInterner,
    symbols: &HashMap<(DefId, Vec<TypeId>), String>,
    layouts: &LayoutTable,
    strings: &IndexMap<String, u32>,
    tags: &HashMap<TypeId, i32>,
    ftable: &HashMap<(DefId, Vec<TypeId>), usize>,
    parent: &MirFunction,
) -> String {
    let (mir, temp) = crate::mir::lower::lower_expr_value(hir, expr, interner);
    let mut out = emit_straight_line_segment(
        &mir, interner, symbols, layouts, strings, tags, ftable, parent,
    );
    let _ = writeln!(out, "     (local.get ${})", temp.0);
    out.push_str("     (local.set $__scratch)\n");
    out
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
            .map(|p| format!(" (param ${} {})", p.0, self.wasm_ty(self.func.local_ty(*p))))
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
        self.line(&format!("(func ${}{}{}", func_symbol(self.func), params, result));

        // Non-parameter locals plus the dispatch program-counter.
        let param_count = self.func.params.len();
        for (i, decl) in self.func.locals.iter().enumerate() {
            if i < param_count {
                continue;
            }
            self.line(&format!("  (local ${} {})", i, self.wasm_ty(decl.ty)));
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
        // Scratch length for `Array.new<T>(len)`: the count is needed for both the allocation size
        // and the zero-fill, so it is materialized once here.
        self.line("  (local $__len i32)");
        // Scratch holding the previous occupant of a reference field/element across a reassignment, so
        // it can be released *after* the new value is stored (deferred release keeps a self-referential
        // `obj.f = g(obj.f)` sound).
        self.line("  (local $__rel i32)");

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
                    &slot_addr,
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
            Statement::InterfaceCall { receiver, iface_id, method_slot, sig, args } => {
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

    /// The heap address of an interned string (0 if not interned — should not happen for strings
    /// surfaced through `strings_in_*`).
    fn string_addr(&self, s: &str) -> u32 {
        self.strings.get(s).copied().unwrap_or(0)
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
        let borrowed =
            matches!(value, Operand::Copy(_) | Operand::Const(Const::Str(_)));
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
        match self.interner.kind(self.interner.strip_nullable(self.func.local_ty(base))) {
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
    fn emit_value_copy(
        &mut self,
        dst: impl Fn(&mut Self),
        src: impl Fn(&mut Self),
        ty: TypeId,
    ) {
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
            let Some(&(off, fty)) = fields.get(i) else { continue };
            let field_addr = |s: &mut Self| {
                dst(s);
                if off > 0 {
                    s.line(&format!("     (i32.const {}) (i32.add)", off));
                }
            };
            if self.interner.is_value_type(fty) {
                let arg = arg.clone();
                self.emit_value_copy(&field_addr, |s| s.emit_operand_addr(&arg), fty);
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
            Rvalue::New { ctor, args, ty: nty, .. } => {
                self.construct_value_new(&dst, *ctor, args, *nty)
            }
            Rvalue::UnionNew { ty: uty, variant, args, .. } => {
                self.construct_value_union(&dst, *uty, *variant, args)
            }
            Rvalue::Call { callee, args } => self.emit_value_sret_call(&dst, callee, args),
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

    fn emit_rvalue(&mut self, rvalue: &Rvalue) {
        match rvalue {
            Rvalue::Use(o) => self.emit_operand(o),
            Rvalue::Binary(op, a, b) => {
                let ty = self.operand_ty(a);
                // String equality compares contents, not pointers, via the runtime `$string_eq`.
                let str_eq = matches!(op, BinOp::Eq | BinOp::Ne)
                    && matches!(
                        self.interner.kind(self.interner.strip_nullable(ty)),
                        TyKind::Prim(PrimTy::String)
                    );
                self.emit_operand(a);
                self.emit_operand(b);
                if str_eq {
                    self.line("     (call $string_eq)");
                    if matches!(op, BinOp::Ne) {
                        self.line("     (i32.eqz)");
                    }
                } else {
                    self.line(&format!("     ({})", self.binop_instr(*op, ty)));
                }
            }
            Rvalue::Unary(op, a) => {
                let ty = self.operand_ty(a);
                match op {
                    UnOp::Neg => {
                        // No `neg` for integers in WASM: 0 - x.
                        if matches!(self.interner.kind(ty), TyKind::Prim(PrimTy::Float | PrimTy::Double)) {
                            self.emit_operand(a);
                            self.line(&format!("     ({}.neg)", self.wasm_ty(ty)));
                        } else {
                            self.line(&format!("     ({}.const 0)", self.wasm_ty(ty)));
                            self.emit_operand(a);
                            self.line(&format!("     ({}.sub)", self.wasm_ty(ty)));
                        }
                    }
                    UnOp::Not => {
                        self.emit_operand(a);
                        self.line("     (i32.eqz)");
                    }
                }
            }
            Rvalue::Call { callee, args } => {
                let sym = self.callee_symbol(callee);
                if let Some(kind) = async_intrinsic_kind(&sym) {
                    self.emit_async_intrinsic(kind, args);
                } else {
                    self.emit_call_args(callee, args);
                    self.line(&format!("     (call ${sym})"));
                }
            }
            Rvalue::IndirectCall { target, args } => {
                for a in args {
                    self.emit_operand(a);
                }
                self.emit_operand(target);
                // The table index (target) is on top of the stack; dispatch through `$__ft` with the
                // signature derived from the target's function type.
                let sig = func_sig(self.interner, self.operand_ty(target))
                    .map(|(name, _, _)| name)
                    .unwrap_or_else(|| "$sig___v".to_string());
                self.line(&format!("     (call_indirect $__ft (type {}))", sig));
            }
            Rvalue::InterfaceCall { receiver, iface_id, method_slot, sig, args, .. } => {
                self.emit_interface_call(receiver, *iface_id, *method_slot, *sig, args);
            }
            Rvalue::FuncRef(callee) => {
                // A function value is its slot index in the module function table.
                let idx = self
                    .func_table
                    .get(&(callee.def, callee.args.clone()))
                    .copied()
                    .unwrap_or(0);
                self.line(&format!("     (i32.const {}) ;; funcref def{}", idx, callee.def.0));
            }
            Rvalue::New { def, ty, ctor, args } => {
                // `$malloc(data_size, tag)` returns a data pointer with refcount 1.
                let info = self
                    .layouts
                    .get(*ty)
                    .map(|l| (l.size, l.fields.iter().map(|f| (f.offset, f.ty)).collect::<Vec<_>>()));
                if let Some((size, fields)) = info {
                    self.line(&format!("     (i32.const {})", size));
                    self.line(&format!("     (i32.const {}) ;; tag", self.type_tag(*ty, *def)));
                    self.line("     (call $malloc)");
                    self.line("     (local.set $__obj)");
                    if let Some(ctor) = ctor {
                        // A user `constructor(this, args...)` sets the fields itself. Reused heap
                        // blocks are not zeroed, so zero every field first (a constructor that leaves a
                        // field unset must observe 0/null), then call it; the object is the result.
                        for &(off, fty) in &fields {
                            self.zero_at_obj(off, fty);
                        }
                        self.line("     (local.get $__obj)");
                        for arg in args {
                            self.emit_operand(arg);
                        }
                        let sym = self.callee_symbol(&crate::mir::Callee {
                            def: *ctor,
                            args: vec![],
                            ret: self.interner.void(),
                        });
                        self.line(&format!("     (call ${})", sym));
                        self.line("     (local.get $__obj)");
                    } else {
                        // Implicit zero-arg default constructor: leave every field at its zero
                        // value. Reused heap blocks are not zeroed, so zero each field explicitly.
                        let _ = args;
                        for &(off, fty) in &fields {
                            self.zero_at_obj(off, fty);
                        }
                        self.line("     (local.get $__obj)");
                    }
                } else {
                    unreachable!("Missing layout for struct allocation");
                }
            }
            Rvalue::UnionNew { def, ty, variant, args } => {
                // A union value is one heap block `[discriminant: i32][payload...]`, sized to the
                // largest variant so any variant fits. `variant` is the discriminant; allocate,
                // write it at offset 0, then store the payload at the variant's field offsets.
                let layout = self.layouts.union(*ty).and_then(|u| {
                    let size = u.size;
                    u.variants
                        .iter()
                        .find(|v| v.discriminant as usize == *variant)
                        .map(|v| (size, v.fields.iter().map(|f| (f.offset, f.ty)).collect::<Vec<_>>()))
                });
                if let Some((size, fields)) = layout {
                    self.line(&format!("     (i32.const {})", size));
                    self.line(&format!("     (i32.const {}) ;; tag", self.type_tag(*ty, *def)));
                    self.line("     (call $malloc)");
                    self.line("     (local.set $__obj)");
                    self.line("     (local.get $__obj)");
                    self.line(&format!("     (i32.const {}) ;; discriminant", variant));
                    self.line("     (i32.store)");
                    for (i, arg) in args.iter().enumerate() {
                        if let Some(&(off, fty)) = fields.get(i) {
                            self.store_at_obj(off, fty, arg);
                        }
                    }
                    self.line("     (local.get $__obj)");
                } else {
                    self.line(&format!(
                        "     (i32.const 0) ;; union def{} variant {} has no layout",
                        def.0, variant
                    ));
                }
            }
            Rvalue::ArrayLit { elem_ty, elems } => {
                // Array block: `[len: i32][elem0][elem1]...`; the length is the first word (matching
                // `ArrayLen`), elements follow at stride `elem_size`.
                let (esize, _) = scalar_size(self.interner, *elem_ty);
                let size = 4 + esize * (elems.len() as u32);
                self.line(&format!("     (i32.const {})", size));
                self.line(&format!("     (i32.const {}) ;; array tag", ARRAY_TAG));
                self.line("     (call $malloc)");
                self.line("     (local.set $__obj)");
                self.line("     (local.get $__obj)");
                self.line(&format!("     (i32.const {})", elems.len()));
                self.line("     (i32.store) ;; length");
                for (i, e) in elems.iter().enumerate() {
                    self.store_at_obj(4 + esize * (i as u32), *elem_ty, e);
                }
                self.line("     (local.get $__obj)");
            }
            Rvalue::ArrayNew { elem_ty, len } => {
                // Block: `[len: i32][elem0..]`, zero-initialized (recycled freelist blocks are not
                // zeroed, and reference-typed releases rely on null slots).
                let (esize, _) = scalar_size(self.interner, *elem_ty);
                self.emit_operand(len);
                self.line("     (local.set $__len)");
                // size = 4 + len * esize
                self.line("     (i32.const 4)");
                self.line("     (local.get $__len)");
                self.line(&format!("     (i32.const {})", esize));
                self.line("     (i32.mul)");
                self.line("     (i32.add)");
                self.line(&format!("     (i32.const {}) ;; array tag", ARRAY_TAG));
                self.line("     (call $malloc)");
                self.line("     (local.set $__obj)");
                self.line("     (local.get $__obj)");
                self.line("     (local.get $__len)");
                self.line("     (i32.store) ;; length");
                // memory.fill(dst = obj+4, 0, len*esize)
                self.line("     (local.get $__obj)");
                self.line("     (i32.const 4)");
                self.line("     (i32.add)");
                self.line("     (i32.const 0)");
                self.line("     (local.get $__len)");
                self.line(&format!("     (i32.const {})", esize));
                self.line("     (i32.mul)");
                self.line("     (memory.fill)");
                self.line("     (local.get $__obj)");
            }
            Rvalue::ArrayLen(o) => {
                self.emit_operand(o);
                self.line("     (i32.load) ;; array length is the first word");
            }
            Rvalue::CharAt(s, i) => {
                self.emit_operand(s);
                self.emit_operand(i);
                self.line("     (call $char_at)");
            }
            Rvalue::Concat(a, b) => {
                self.emit_operand(a);
                self.emit_operand(b);
                self.line("     (call $concat_strings)");
            }
            Rvalue::ToString(o) => {
                self.emit_operand(o);
                let oty = self.operand_ty(o);
                // A value struct/union is addressed inline (no heap tag header), so its `to_string`
                // is dispatched statically to the concrete `$<Type>_to_string` rather than routed
                // through the tag-dispatching `$object_to_string`.
                if self.interner.is_value_type(oty) {
                    if let Some(name) = self.value_name(oty) {
                        self.line(&format!("     (call ${}_to_string)", name));
                        return;
                    }
                }
                // A `string` is already its own `to_string`; every other type has a formatter.
                if let Some(call) = value_to_string_call(self.interner, oty) {
                    self.line(&format!("     (call {})", call));
                }
            }
            Rvalue::EnumName { value, arms } => {
                let empty = self.string_addr("");
                self.emit_operand(value);
                self.line("     (local.set $__len)");
                // Nested `value == disc ? strptr : (...)`, terminating in the empty string.
                for (disc, name) in arms {
                    let ptr = self.string_addr(name);
                    self.line("     (local.get $__len)");
                    self.line(&format!("     (i32.const {})", disc));
                    self.line("     (i32.eq)");
                    self.line("     (if (result i32)");
                    self.line(&format!("      (then (i32.const {}))", ptr));
                    self.line("      (else");
                }
                self.line(&format!("     (i32.const {})", empty));
                for _ in arms {
                    self.line("     ))");
                }
            }
            Rvalue::HashCode(o) => {
                self.emit_operand(o);
                let oty = self.operand_ty(o);
                if self.interner.is_value_type(oty) {
                    if let Some(name) = self.value_name(oty) {
                        self.line(&format!("     (call ${}_hash_code)", name));
                        return;
                    }
                }
                match self.interner.kind(self.interner.strip_nullable(oty)) {
                    // Integer-family values (and enums) are their own hash.
                    TyKind::Prim(
                        PrimTy::Int | PrimTy::UInt | PrimTy::Bool | PrimTy::Char | PrimTy::Byte,
                    )
                    | TyKind::Enum(_) => {}
                    TyKind::Prim(PrimTy::Long | PrimTy::ULong) => self.line("     (call $hash_long)"),
                    TyKind::Prim(PrimTy::Float) => self.line("     (i32.reinterpret_f32)"),
                    TyKind::Prim(PrimTy::Double) => self.line("     (call $hash_double)"),
                    TyKind::Prim(PrimTy::String) => self.line("     (call $hash_string)"),
                    _ => self.line("     (call $object_hash_code)"),
                }
            }
            Rvalue::StrLen(o) => {
                self.emit_operand(o);
                self.line("     (call $strlen) ;; strings are NUL-terminated");
            }
            Rvalue::Cast(o, from, to) => self.emit_cast(o, *from, *to),
            Rvalue::IsType(o, target) => {
                self.emit_operand(o);
                self.line("     (call $object_tag)");
                let tag = runtime_tag_for(self.interner, self.tags, *target).unwrap_or(0);
                self.line(&format!("     (i32.const {})", tag));
                self.line("     (i32.eq)");
            }
            Rvalue::Discriminant(o) => {
                // The discriminant is the `i32` at offset 0 of the union block.
                self.emit_operand(o);
                self.line("     (i32.load) ;; union discriminant");
            }
            Rvalue::UnionField { base, ty, variant, field } => {
                let slot = self.layouts.union(*ty).and_then(|u| {
                    u.variants
                        .iter()
                        .find(|v| v.discriminant as usize == *variant)
                        .and_then(|v| v.fields.get(*field))
                        .map(|f| (f.offset, f.ty))
                });
                if let Some((off, fty)) = slot {
                    self.emit_operand(base);
                    if off > 0 {
                        self.line(&format!("     (i32.const {})", off));
                        self.line("     (i32.add)");
                    }
                    // A value-struct payload is addressed inline (its bytes live in the union block),
                    // so reading it yields the payload address rather than a load.
                    if !self.interner.is_value_type(fty) {
                        self.line(&format!("     ({})", self.load_instr(fty)));
                    }
                } else {
                    unreachable!("Missing layout for union payload");
                }
            }
        }
    }

    fn emit_cast(&mut self, o: &Operand, from: TypeId, to: TypeId) {
        let from_prim = prim_of(self.interner, from);
        let to_prim = prim_of(self.interner, to);
        let to_is_object = matches!(
            self.interner.kind(self.interner.strip_nullable(to)),
            TyKind::Object
        );
        let from_is_object = matches!(
            self.interner.kind(self.interner.strip_nullable(from)),
            TyKind::Object
        );
        // Boxing a primitive into `object` (reference types are already pointers → identity).
        if to_is_object {
            self.emit_operand(o);
            if let Some(boxfn) = from_prim.and_then(box_fn_for) {
                self.line(&format!("     (call {})", boxfn));
            }
            return;
        }
        // Unboxing `object` to a primitive (or leaving a reference pointer as-is).
        if from_is_object {
            self.emit_operand(o);
            if let Some(unboxfn) = to_prim.and_then(unbox_fn_for) {
                self.line(&format!("     (call {})", unboxfn));
            }
            return;
        }
        self.emit_operand(o);
        self.emit_numeric_conv(from, to);
        // Narrowing to `byte` (which shares the `i32` WASM type with `int`/`uint`, so `numeric_conv`
        // is a no-op) must wrap into the [0, 255] range explicitly (C-style truncation).
        if matches!(to_prim, Some(PrimTy::Byte)) {
            self.line("     (i32.const 255)");
            self.line("     (i32.and)");
        }
    }

    /// Emits a call's arguments, applying implicit numeric widening to each so a narrower argument
    /// (e.g. an `int`/`float` passed to a `double` parameter) matches the callee's WASM signature.
    /// Falls back to a plain push when the callee's parameter types are unknown (imports/intrinsics).
    fn emit_call_args(&mut self, callee: &crate::mir::Callee, args: &[Operand]) {
        let params = self.sigs.get(&(callee.def, callee.args.clone())).cloned();
        for (i, a) in args.iter().enumerate() {
            self.emit_operand(a);
            if let Some(pty) = params.as_ref().and_then(|p| p.get(i)) {
                self.emit_numeric_conv(self.operand_ty(a), *pty);
            }
        }
    }

    /// Emits a dynamic interface method call. The receiver is pushed as argument 0, then the real
    /// arguments (widened to the interface method's declared parameter types), then control transfers
    /// to the per-`(interface, method)` dispatch trampoline which looks the concrete implementation up
    /// in the tag-indexed itable and forwards through `$__ft`. The trampoline leaves the result (if
    /// any) on the stack.
    fn emit_interface_call(
        &mut self,
        receiver: &Operand,
        iface_id: usize,
        method_slot: usize,
        sig: TypeId,
        args: &[Operand],
    ) {
        let param_tys: Vec<TypeId> = match self.interner.kind(sig) {
            TyKind::Func(params, _) => params.clone(),
            _ => Vec::new(),
        };
        self.emit_operand(receiver);
        for (i, a) in args.iter().enumerate() {
            self.emit_operand(a);
            // param_tys[0] is the receiver (`this`); real args start at index 1.
            if let Some(pty) = param_tys.get(i + 1) {
                self.emit_numeric_conv(self.operand_ty(a), *pty);
            }
        }
        self.line(&format!(
            "     (call ${})",
            iface_dispatch_symbol(iface_id, method_slot)
        ));
    }

    /// Emits the WASM numeric conversion instruction to turn a value of type `from` (already on the
    /// stack) into type `to`, if their WASM value types differ (a no-op otherwise). Shared by explicit
    /// `Cast` and the implicit widening applied to call arguments.
    fn emit_numeric_conv(&mut self, from: TypeId, to: TypeId) {
        let (fw, tw) = (self.wasm_ty(from), self.wasm_ty(to));
        if fw != tw {
            // Numeric conversions between the four WASM value types. Integer/float conversions carry
            // the signedness of the *integer* side (the target for float→int, the source otherwise);
            // saturating float→int truncation matches C-style cast semantics (no trap on overflow/NaN).
            let (fw, tw) = (fw.as_str(), tw.as_str());
            let int_signed = |ty: TypeId| {
                !matches!(
                    self.interner.kind(self.interner.strip_nullable(ty)),
                    TyKind::Prim(PrimTy::UInt | PrimTy::ULong | PrimTy::Byte)
                )
            };
            let instr = match (fw, tw) {
                ("i32", "i64") => if int_signed(from) { "i64.extend_i32_s" } else { "i64.extend_i32_u" },
                ("i64", "i32") => "i32.wrap_i64",
                ("i32", "f32") => if int_signed(from) { "f32.convert_i32_s" } else { "f32.convert_i32_u" },
                ("i32", "f64") => if int_signed(from) { "f64.convert_i32_s" } else { "f64.convert_i32_u" },
                ("i64", "f32") => if int_signed(from) { "f32.convert_i64_s" } else { "f32.convert_i64_u" },
                ("i64", "f64") => if int_signed(from) { "f64.convert_i64_s" } else { "f64.convert_i64_u" },
                ("f32", "f64") => "f64.promote_f32",
                ("f64", "f32") => "f32.demote_f64",
                ("f32", "i32") => if int_signed(to) { "i32.trunc_sat_f32_s" } else { "i32.trunc_sat_f32_u" },
                ("f64", "i32") => if int_signed(to) { "i32.trunc_sat_f64_s" } else { "i32.trunc_sat_f64_u" },
                ("f32", "i64") => if int_signed(to) { "i64.trunc_sat_f32_s" } else { "i64.trunc_sat_f32_u" },
                ("f64", "i64") => if int_signed(to) { "i64.trunc_sat_f64_s" } else { "i64.trunc_sat_f64_u" },
                _ => "nop",
            };
            self.line(&format!("     ({})", instr));
        }
    }

    fn emit_terminator(&mut self, t: &Terminator) {
        match t {
            Terminator::Goto(b) => self.goto(*b),
            Terminator::If { cond, then_blk, else_blk } => {
                self.emit_operand(cond);
                self.line("     (if (then");
                self.goto(*then_blk);
                self.line("     ) (else");
                self.goto(*else_blk);
                self.line("     ))");
            }
            Terminator::Switch { value, targets, default } => {
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
            Terminator::Unreachable => self.line("     (unreachable)"),
            Terminator::AsyncComplete(_) => self.line("     (unreachable) ;; async in sync fn"),
        }
    }

    /// Terminator emission for async poll segments (completes the task instead of returning). Only
    /// value-carrying/`AsyncComplete` terminators reach here; a void fall-through is elided by the
    /// caller so control continues to the segment's suspend/complete handling.
    fn emit_poll_terminator(&mut self, t: &Terminator) {
        match t {
            Terminator::AsyncComplete(v) => {
                let v = v.clone();
                self.emit_poll_complete(v.as_ref());
            }
            // A value `return x;` inside an async body lowers to `AsyncComplete`, but handle the plain
            // form too (complete the coroutine with the value rather than returning it from `poll`).
            Terminator::Return(Some(o)) => {
                let o = o.clone();
                self.emit_poll_complete(Some(&o));
            }
            Terminator::Return(None) => self.emit_poll_complete(None),
            _ => {}
        }
    }

    /// Completes the current coroutine: releases the parent's reference locals, then
    /// `$dream_complete($self, value)` and returns `0` (the poll result).
    fn emit_poll_complete(&mut self, value: Option<&Operand>) {
        if let Some(parent) = self.async_parent {
            for (i, decl) in parent.locals.iter().enumerate() {
                if self.interner.is_reference(decl.ty) {
                    let call = release_call(self.interner, self.layouts, decl.ty);
                    self.line(&format!("     (local.get ${i})"));
                    self.line(&format!("     (call {call})"));
                }
            }
        }
        self.line("     (local.get $self)");
        match value {
            Some(v) => self.emit_operand(v),
            None => self.line("     (i32.const 0)"),
        }
        self.line("     (call $dream_complete)");
        self.line("     (i32.const 0)");
        self.line("     (return)");
    }

    /// Emits a plain async poll segment's body. A single-block segment is emitted inline; its void
    /// fall-through terminator is elided so control continues to the segment's own suspend/complete
    /// handling in `emit_async_function`. A multi-block segment (the plain code contained an
    /// `if`/loop/`match`, or a suspend expression with control flow) is emitted through a `$__pc`
    /// dispatch loop wrapped in `$__segexit`: CFG edges re-dispatch, completions run
    /// `$dream_complete`, and a void fall-through/`unreachable` tail `br`s to `$__segexit` so the
    /// code the caller appends after the segment runs next.
    fn emit_poll_segment_body(&mut self) {
        let n = self.func.blocks.len();
        if n <= 1 {
            let block = self.func.block(self.func.entry);
            for stmt in &block.stmts {
                self.emit_stmt(stmt);
            }
            match &block.terminator {
                Terminator::AsyncComplete(None)
                | Terminator::Return(None)
                | Terminator::Unreachable => {}
                other => self.emit_poll_terminator(other),
            }
            return;
        }
        self.line("     (block $__segexit");
        self.line(&format!("      (i32.const {})", self.func.entry.0));
        self.line("      (local.set $__pc)");
        self.line("      (loop $__loop");
        for i in (0..n).rev() {
            self.line(&format!("       (block $bb{}", i));
        }
        let labels: String = (0..n).map(|i| format!("$bb{} ", i)).collect();
        let default = format!("$bb{}", n.saturating_sub(1));
        self.line(&format!("        (br_table {}{} (local.get $__pc))", labels, default));
        for i in 0..n {
            self.line(&format!("       ) ;; bb{} body", i));
            let block = self.func.block(crate::mir::BlockId(i as u32));
            for stmt in &block.stmts {
                self.emit_stmt(stmt);
            }
            self.emit_poll_cfg_terminator(&block.terminator);
        }
        self.line("      )"); // loop
        self.line("     )"); // $__segexit
    }

    /// Terminator emission inside a multi-block poll segment: CFG edges dispatch through `$__pc`
    /// (via [`Self::emit_terminator`]'s `goto`), completions run `$dream_complete`, and a void
    /// fall-through/`unreachable` tail exits the dispatch loop so the segment's trailing code runs.
    fn emit_poll_cfg_terminator(&mut self, t: &Terminator) {
        match t {
            Terminator::Goto(_) | Terminator::If { .. } | Terminator::Switch { .. } => {
                self.emit_terminator(t)
            }
            Terminator::AsyncComplete(_) | Terminator::Return(Some(_)) => {
                self.emit_poll_terminator(t)
            }
            Terminator::Return(None) | Terminator::Unreachable => {
                self.line("     (br $__segexit)")
            }
        }
    }

    /// Emits `sleep` / `Promise.all|any|race`, leaving a `Future` pointer on the stack.
    fn emit_async_intrinsic(&mut self, kind: &str, args: &[Operand]) {
        use crate::intrinsics;
        match kind {
            intrinsics::SLEEP => {
                self.emit_operand(&args[0]);
                self.line("     (local.set $__scratch)");
                self.line("     (i32.const 56) ;; F_SLOTS");
                self.line("     (i32.const -1)");
                self.line("     (i32.const 1) ;; KIND_HOST");
                self.line("     (call $dream_new_future)");
                self.line("     (local.tee $__obj)");
                self.line("     (local.get $__scratch)");
                self.line("     (call $dream_set_timer)");
                self.line("     (local.get $__obj)");
            }
            intrinsics::PROMISE_ALL => {
                self.emit_operand(&args[0]);
                self.line("     (call $dream_all)");
            }
            intrinsics::PROMISE_ANY | intrinsics::PROMISE_RACE => {
                self.emit_operand(&args[0]);
                self.line("     (call $dream_any)");
            }
            _ => {}
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
            Operand::Copy(Place::Index { base, .. }) => {
                self.array_elem_ty(*base).unwrap_or_else(|| self.func.local_ty(*base))
            }
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