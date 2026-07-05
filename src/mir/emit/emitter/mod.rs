//! Emits one MIR function as WAT text. Core function-shell emission (locals/dispatch-loop scaffold)
//! plus the low-level operand/type helpers shared by every other emitter submodule live here; the
//! larger emission concerns are split out:
//! - [`statements`]: `Statement` emission (assignment, print, retain/release, calls) and the
//!   place-store helpers (retain-on-store, deferred release of an overwritten reference).
//! - [`value_struct`]: the value(`struct`)/value-union inline (non-heap) representation — shadow
//!   frame prologue/teardown, in-place construction, byte-copy, and sret-call helpers.
//! - [`terminator`]: `Terminator` emission (branches, returns, tail calls) and the dynamic `js` call
//!   marshaling helper.
//! - [`async_ops`]: async-coroutine poll emission (split out previously).
//! - [`rvalue`]: `Rvalue` (expression) emission (split out previously).

use super::*;
use crate::mir::async_emit::{slot_load, slot_store, AsyncSlots, F_AWAITING, F_RESULT, F_STATE};
use crate::mir::emit::valuetype::{ValueFrame, ValueLocalKind};
use std::collections::HashSet;

mod async_ops;
mod rvalue;
mod statements;
mod terminator;
mod value_struct;

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
        None,
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
    debug_fn: Option<&crate::mir::emit::debug_map::DebugFunction>,
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
        debug_fn,
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
    debug_fn: Option<&crate::mir::emit::debug_map::DebugFunction>,
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
        debug_fn,
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
    /// Debug-info metadata for this function when compiled with source-level debug-info (line hooks
    /// + local spilling). `None` disables all instrumentation (release builds, async bodies).
    debug_fn: Option<&'a crate::mir::emit::debug_map::DebugFunction>,
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
        // Debug-info: announce entry into this function so the debugger can push a call-stack frame.
        if let Some(dbg) = self.debug_fn {
            self.line(&format!("  (call $__dbg_enter (i32.const {}))", dbg.id));
        }
        self.emit_dispatch();
        self.line(")");
    }

    /// Emits the `dream_debug.exit` hook (pops the debugger's call-stack frame) right before a return,
    /// when debug-info is on. Placed at every real return site so the shadow call stack stays
    /// balanced regardless of which path exits the function.
    fn emit_debug_exit(&mut self) {
        if let Some(dbg) = self.debug_fn {
            self.line(&format!("     (call $__dbg_exit (i32.const {}))", dbg.id));
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
            crate::internal_error!("string literal {:?} was not interned before codegen", s)
        })
    }

    /// Pushes the address of `base.field` (`base + offset`) onto the stack.
    fn field_addr(&mut self, base: crate::mir::Local, offset: u32) {
        self.line(&format!("     (local.get ${})", base.0));
        if offset > 0 {
            self.line(&format!("     (i32.const {})", offset));
            self.line("     (i32.add)");
        }
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
                    crate::internal_error!(
                        "missing field layout for read (base {:?}, field {})",
                        base,
                        field
                    );
                }
            }
            Operand::Copy(Place::Index { base, index }) => {
                if let Some(ety) = self.array_elem_ty(*base) {
                    self.elem_addr(*base, ety, index);
                    if !self.interner.is_value_type(ety) {
                        self.line(&format!("     ({})", self.load_instr(ety)));
                    }
                } else {
                    crate::internal_error!("missing array element type for read (base {:?})", base);
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
                None => crate::internal_error!("missing interned string: {}", s),
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
