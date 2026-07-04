//! Codegen support for inline value (`struct`) types.
//!
//! A value struct is stored *inline* (its bytes live directly in a shadow-stack frame slot, a
//! container field/element, or a union payload) rather than as a heap-allocated, reference-counted
//! pointer. At the WASM level a value-struct local is an `i32` holding the **address** of its
//! storage; reading such a place yields that address (never a load), and moving a value struct into
//! a new location performs a byte-wise copy plus a recursive retain of any reference fields.
//!
//! This module computes, per function, the shadow-frame layout and the ownership classification of
//! each value-struct local, and emits the per-type retain/drop glue (`$__vs_retain_<T>` /
//! `$__vs_drop_<T>`) that keeps reference fields embedded inside value structs balanced.

use super::*;
use crate::mir::{Local, MirFunction, Operand, Place, Rvalue, Statement};
use std::collections::{HashMap, HashSet};

/// Ownership classification of a value(`struct`)-typed local, driving shadow-frame codegen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ValueLocalKind {
    /// A value parameter. It arrives as an `i32` pointer to the *caller's* value; to preserve copy
    /// semantics the callee owns a private copy, so it gets a frame slot initialized by copying the
    /// incoming bytes (retaining reference fields) on entry, and is dropped at scope exit.
    Param,
    /// A synthetic alias to a value place (the base of a nested field/index access, or a value
    /// argument): its `i32` holds the address of an existing value. No slot; not dropped.
    Borrow,
    /// Owns its storage: gets a zeroed shadow-frame slot, is copied into on assignment, and is
    /// dropped (reference fields released) at scope exit.
    Owning,
}

/// Per-function shadow-frame layout for inline value(`struct`) locals.
pub(super) struct ValueFrame {
    /// Byte offset within the frame of each owning value local's storage.
    slots: HashMap<Local, u32>,
    /// Ownership classification of every value-struct local (others are absent).
    kinds: HashMap<Local, ValueLocalKind>,
    /// Total frame size in bytes (0 when the function has no owning value locals).
    pub size: u32,
}

impl ValueFrame {
    /// Classifies every value-struct local and lays out a shadow-frame slot for each owning one.
    pub fn compute(func: &MirFunction, interner: &TypeInterner) -> ValueFrame {
        let param_count = func.params.len();

        // Collect the defining rvalues of each local so an alias temp (a synthetic local whose only
        // definitions copy a value place) can be told apart from an owning binding.
        let mut defs: HashMap<u32, Vec<&Rvalue>> = HashMap::new();
        for block in &func.blocks {
            for stmt in &block.stmts {
                if let Statement::Assign(Place::Local(l), rv) = stmt {
                    defs.entry(l.0).or_default().push(rv);
                }
            }
        }

        let mut kinds = HashMap::new();
        let mut slots = HashMap::new();
        let mut size = 0u32;
        for (i, decl) in func.locals.iter().enumerate() {
            if !interner.is_value_type(decl.ty) {
                continue;
            }
            let local = Local(i as u32);
            let kind = if i < param_count {
                // The receiver `this` is borrowed in place (methods/constructors mutate the caller's
                // instance), so it takes no private copy; other value params are copied for value
                // semantics.
                if decl.name.as_deref() == Some("this") {
                    ValueLocalKind::Borrow
                } else {
                    ValueLocalKind::Param
                }
            } else {
                let alias = decl.name.is_none()
                    && defs
                        .get(&(i as u32))
                        .map(|ds| !ds.is_empty() && ds.iter().all(|rv| is_value_place_copy(rv)))
                        .unwrap_or(false);
                if alias {
                    ValueLocalKind::Borrow
                } else {
                    ValueLocalKind::Owning
                }
            };
            // Params and owning locals each get a private frame slot (params are copied into theirs on
            // entry, owning locals are zeroed); a borrow alias reuses an existing value's storage.
            if matches!(kind, ValueLocalKind::Owning | ValueLocalKind::Param) {
                let (sz, al) = crate::hir::scalar_size(interner, decl.ty);
                let rem = size % al;
                if rem != 0 {
                    size += al - rem;
                }
                slots.insert(local, size);
                size += sz;
            }
            kinds.insert(local, kind);
        }
        // Keep the frame 8-byte aligned so a `double` field in the first slot stays naturally aligned.
        if !size.is_multiple_of(8) {
            size += 8 - size % 8;
        }
        ValueFrame { slots, kinds, size }
    }

    pub fn kind(&self, l: Local) -> Option<ValueLocalKind> {
        self.kinds.get(&l).copied()
    }

    /// Every value local that owns a frame slot (params and owning locals), with its frame offset,
    /// ordered by offset (deterministic emission). These are the locals dropped at scope exit.
    pub fn owning_slots(&self) -> Vec<(Local, u32)> {
        let mut v: Vec<(Local, u32)> = self.slots.iter().map(|(l, o)| (*l, *o)).collect();
        v.sort_by_key(|(_, o)| *o);
        v
    }
}

/// True when `rv` copies a value place (`Use(Copy(local|field|index))`) — the shape of an alias temp
/// standing in for the base of a nested access or a by-value argument.
fn is_value_place_copy(rv: &Rvalue) -> bool {
    matches!(
        rv,
        Rvalue::Use(Operand::Copy(
            Place::Local(_) | Place::Field { .. } | Place::Index { .. }
        ))
    )
}

/// The `$__vs_retain_<T>` symbol: retains (increments) every reference reachable *by value* inside a
/// value struct after a byte-wise copy, so the copy owns its own references.
pub(super) fn vs_retain_sym(name: &str) -> String {
    format!("$__vs_retain_{}", name)
}

/// The `$__vs_drop_<T>` symbol: runs `del()` (if any) then releases every reference reachable by
/// value inside a value struct, when an owning value goes out of scope or is overwritten.
pub(super) fn vs_drop_sym(name: &str) -> String {
    format!("$__vs_drop_{}", name)
}

/// The set of value-struct types that require retain/drop glue: those that (transitively) embed a
/// reference field, or declare a `del()` destructor. Purely-scalar value structs need none, so their
/// copies and drops are plain byte moves with no bookkeeping.
pub(super) fn value_glue_types(mir: &crate::mir::Mir, interner: &TypeInterner) -> HashSet<TypeId> {
    let fn_names: HashSet<&str> = mir.functions.iter().map(|f| f.name.as_str()).collect();
    let mut out = HashSet::new();
    let struct_keys: Vec<TypeId> = mir.layouts.structs.keys().copied().collect();
    for ty in struct_keys {
        if interner.is_value_type(ty) {
            needs_glue(ty, mir, interner, &fn_names, &mut out, &mut HashSet::new());
        }
    }
    // Value unions can also embed references (via a value-struct payload), so they too may need glue.
    let union_keys: Vec<TypeId> = mir.layouts.unions.keys().copied().collect();
    for ty in union_keys {
        if interner.is_value_union(ty) {
            needs_glue(ty, mir, interner, &fn_names, &mut out, &mut HashSet::new());
        }
    }
    out
}

/// Determines whether value type `ty` (a value struct or value union) needs glue, memoizing the
/// answer into `out` (the set of glue-requiring types). `visiting` guards the recursion (value-type
/// cycles are a rejected error).
fn needs_glue(
    ty: TypeId,
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    fn_names: &HashSet<&str>,
    out: &mut HashSet<TypeId>,
    visiting: &mut HashSet<TypeId>,
) -> bool {
    let ty = interner.strip_nullable(ty);
    if out.contains(&ty) {
        return true;
    }
    if !visiting.insert(ty) {
        return false;
    }
    // A value union needs glue when any variant payload is a reference or a glue-needing value type.
    if interner.is_value_union(ty) {
        let mut needs = false;
        if let Some(u) = mir.layouts.unions.get(&ty) {
            for v in &u.variants {
                for f in &v.fields {
                    if interner.is_reference(f.ty) {
                        needs = true;
                    } else if interner.is_value_type(f.ty)
                        && needs_glue(f.ty, mir, interner, fn_names, out, visiting)
                    {
                        needs = true;
                    }
                }
            }
        }
        visiting.remove(&ty);
        if needs {
            out.insert(ty);
        }
        return needs;
    }
    let Some(layout) = mir.layouts.structs.get(&ty) else {
        visiting.remove(&ty);
        return false;
    };
    let mut needs = fn_names.contains(format!("{}_del", layout.name).as_str());
    for f in &layout.fields {
        if interner.is_reference(f.ty) {
            needs = true;
        } else if interner.is_value_type(f.ty)
            && needs_glue(f.ty, mir, interner, fn_names, out, visiting)
        {
            needs = true;
        }
    }
    visiting.remove(&ty);
    if needs {
        out.insert(ty);
    }
    needs
}

/// Emits the per-value-struct retain/drop glue for every type in `glue`.
pub(super) fn emit_value_glue(
    out: &mut String,
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    glue: &HashSet<TypeId>,
) {
    let fn_names: HashSet<&str> = mir.functions.iter().map(|f| f.name.as_str()).collect();
    // Deterministic order: layout-table (struct) order, filtered to the glue set.
    for (ty, layout) in &mir.layouts.structs {
        if !glue.contains(ty) {
            continue;
        }
        // `$__vs_retain_<T>(ptr)`: retain each reference field; recurse into value fields.
        let _ = writeln!(
            out,
            "(func {} (param $ptr i32)",
            vs_retain_sym(&layout.name)
        );
        for f in &layout.fields {
            emit_field_glue(out, mir, interner, glue, f, GlueOp::Retain);
        }
        out.push_str(")\n");

        // `$__vs_drop_<T>(ptr)`: run `del()` (if any), then release each reference field / recurse.
        let _ = writeln!(out, "(func {} (param $ptr i32)", vs_drop_sym(&layout.name));
        let del = format!("{}_del", layout.name);
        if fn_names.contains(del.as_str()) {
            let _ = writeln!(out, "  (local.get $ptr) (call ${})", del);
        }
        for f in &layout.fields {
            emit_field_glue(out, mir, interner, glue, f, GlueOp::Drop);
        }
        out.push_str(")\n");
    }

    // Value-union glue is variant-aware: the discriminant at offset 0 selects which payload fields
    // are live, so each retain/drop guards its field work on `discriminant == variant`.
    for (ty, layout) in &mir.layouts.unions {
        if !glue.contains(ty) {
            continue;
        }
        for op in [GlueOp::Retain, GlueOp::Drop] {
            let sym = match op {
                GlueOp::Retain => vs_retain_sym(&layout.name),
                GlueOp::Drop => vs_drop_sym(&layout.name),
            };
            let _ = writeln!(out, "(func {} (param $ptr i32)", sym);
            for v in &layout.variants {
                let live: Vec<&crate::hir::FieldLayout> = v
                    .fields
                    .iter()
                    .filter(|f| {
                        interner.is_reference(f.ty)
                            || (interner.is_value_type(f.ty)
                                && glue.contains(&interner.strip_nullable(f.ty)))
                    })
                    .collect();
                if live.is_empty() {
                    continue;
                }
                let _ = writeln!(
                    out,
                    "  (local.get $ptr) (i32.load) (i32.const {}) (i32.eq) (if (then",
                    v.discriminant
                );
                for f in live {
                    emit_field_glue(out, mir, interner, glue, f, op);
                }
                out.push_str("  ))\n");
            }
            out.push_str(")\n");
        }
    }
}

#[derive(Clone, Copy)]
enum GlueOp {
    Retain,
    Drop,
}

/// Emits the retain or release of one field `f` at `$ptr + offset`: a reference field is
/// retained/released by pointer; a value-struct field recurses into its own glue by address; a
/// scalar field needs nothing.
fn emit_field_glue(
    out: &mut String,
    mir: &crate::mir::Mir,
    interner: &TypeInterner,
    glue: &HashSet<TypeId>,
    f: &crate::hir::FieldLayout,
    op: GlueOp,
) {
    let addr = |out: &mut String| {
        out.push_str("  (local.get $ptr)");
        if f.offset > 0 {
            let _ = write!(out, " (i32.const {}) (i32.add)", f.offset);
        }
    };
    if interner.is_reference(f.ty) {
        addr(out);
        match op {
            GlueOp::Retain => out.push_str(" (i32.load) (call $retain)\n"),
            GlueOp::Drop => {
                let _ = writeln!(
                    out,
                    " (i32.load) (call {})",
                    release_call(interner, &mir.layouts, f.ty)
                );
            }
        }
    } else if interner.is_value_type(f.ty) && glue.contains(&interner.strip_nullable(f.ty)) {
        let stripped = interner.strip_nullable(f.ty);
        // A nested value field is either a value struct or a value union; resolve its glue name from
        // whichever layout table holds it.
        let name = mir
            .layouts
            .structs
            .get(&stripped)
            .map(|l| l.name.clone())
            .or_else(|| mir.layouts.unions.get(&stripped).map(|u| u.name.clone()));
        if let Some(name) = name {
            addr(out);
            match op {
                GlueOp::Retain => {
                    let _ = writeln!(out, " (call {})", vs_retain_sym(&name));
                }
                GlueOp::Drop => {
                    let _ = writeln!(out, " (call {})", vs_drop_sym(&name));
                }
            }
        }
    }
}
