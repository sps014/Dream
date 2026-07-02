use super::*;

impl<'a> Analyzer<'a> {
    /// Turns on HIR collection so a top-level variable's initializer expression is captured while it
    /// is analyzed. There is no enclosing function, so there are no locals/blocks — only the top
    /// expression's HIR is wanted. Paired with [`Self::hir_global_init_finish`].
    pub(in crate::semantics::analyzer) fn hir_global_init_begin(&mut self) {
        self.hir.collecting = true;
        self.hir.ok = true;
        self.hir.last = None;
    }

    /// Stores the captured initializer for global `name` (if it was fully representable) and turns
    /// collection back off.
    pub(in crate::semantics::analyzer) fn hir_global_init_finish(&mut self, name: &str) {
        if self.hir.collecting && self.hir.ok {
            if let Some(init) = self.hir.last.take() {
                self.hir.pending_global_inits.insert(name.to_string(), init);
            }
        }
        self.hir.collecting = false;
        self.hir.last = None;
    }

    /// Registers one top-level variable's HIR slot as it is analyzed (in declaration order), so a
    /// *later* global's initializer can resolve an *earlier* global to a [`Binding::Global`]. The
    /// slot `id` must equal the variable's index in [`Analyzer::globals`]. The initializer captured
    /// by [`Self::hir_global_init_finish`] (if representable) is attached to the surfaced [`HGlobal`].
    pub(in crate::semantics::analyzer) fn hir_register_global(&mut self, name: &str, type_str: &str, is_const: bool) {
        let ty = self.type_ctx.lower_str(type_str);
        let id = GlobalId(self.hir.globals.len() as u32);
        self.hir.globals.insert(name.to_string(), (id, ty));
        let init = self.hir.pending_global_inits.shift_remove(name);
        self.hir.global_decls.push(HGlobal {
            id,
            name: name.to_string(),
            ty,
            is_const,
            init,
        });
    }

    /// Builds the [`crate::hir::LayoutTable`] from the analyzed struct and union tables: each struct's
    /// `DefId` maps to its field offsets/sizes, and each union's `DefId` to its per-variant
    /// discriminant + payload offsets, so the backend can lower `obj.field` reads and `new`/variant
    /// construction to concrete loads/stores.
    pub(in crate::semantics::analyzer) fn hir_build_layouts(&mut self) -> crate::hir::LayoutTable {
        use crate::hir::{FieldLayout, LayoutTable, TypeLayout, UnionLayout, UnionVariant};
        // Snapshot field types in declaration order first, so `type_ctx` can be re-borrowed mutably
        // for lowering without aliasing the struct/union-table borrows.
        // Discriminated unions are also registered in the struct table (for tagging/release), but they
        // get a variant-aware layout + `to_string` from the union table below — so exclude them here to
        // avoid a duplicate (empty) struct layout and a duplicate `$<Union>_to_string`.
        let struct_snapshot: Vec<(String, Vec<(String, Type)>)> = self
            .struct_table
            .structs
            .iter()
            .filter(|(name, _)| !self.union_table.contains_key(name.as_str()))
            .map(|(name, info)| {
                let fields =
                    info.fields.iter().map(|(fname, f)| (fname.clone(), f.type_.clone())).collect();
                (name.clone(), fields)
            })
            .collect();
        // (union name, block size, [(variant name, discriminant, [(field name, offset, field type)])]).
        type VariantSnap = (String, i32, Vec<(String, u32, Type)>);
        let union_snapshot: Vec<(String, u32, Vec<VariantSnap>)> = self
            .union_table
            .iter()
            .map(|(name, info)| {
                let variants = info
                    .variants
                    .iter()
                    .map(|v| {
                        let fields = v
                            .fields
                            .iter()
                            .map(|f| (f.name.clone(), f.offset as u32, f.type_.clone()))
                            .collect();
                        (v.name.clone(), v.discriminant, fields)
                    })
                    .collect();
                (name.clone(), info.size as u32, variants)
            })
            .collect();

        let mut layouts = LayoutTable::default();
        // Lower every struct's fields to interned ids up front. Keyed by the struct's interned type id
        // (`lower_str` canonicalizes both plain names and mangled generic instances like `Box_int` to
        // `struct_ty(def, args)`), so each monomorphization gets its own layout.
        let mut lowered: Vec<(TypeId, String, Vec<(String, TypeId)>)> =
            Vec::with_capacity(struct_snapshot.len());
        for (name, fields) in struct_snapshot {
            let ty = self.type_ctx.lower_str(&name);
            let defs: Vec<(String, TypeId)> =
                fields.iter().map(|(fname, t)| (fname.clone(), self.type_ctx.lower(t))).collect();
            lowered.push((ty, name, defs));
        }
        // Value (`struct`) types are stored inline, so their footprint must be known before any layout
        // (or an enclosing struct/array/union) can size them. Compute each value struct's inline
        // (size, align) recursively — a value field contributes its full footprint; a reference field
        // contributes a 4-byte pointer — and record it on the interner so `scalar_size` resolves it.
        let field_map: std::collections::HashMap<TypeId, Vec<TypeId>> = lowered
            .iter()
            .map(|(ty, _, defs)| (*ty, defs.iter().map(|(_, t)| *t).collect()))
            .collect();
        // A value union's inline footprint is `discriminant(4) + max value-aware variant payload`.
        // Collect each value union's variants as lists of payload field ids so the unified layout
        // computation can size value structs and value unions that embed one another.
        let mut union_field_map: std::collections::HashMap<TypeId, Vec<Vec<TypeId>>> =
            std::collections::HashMap::new();
        for (name, _size, variants) in &union_snapshot {
            let ty = self.type_ctx.lower_str(name);
            if !self.type_ctx.interner.is_value_union(ty) {
                continue;
            }
            let vs = variants
                .iter()
                .map(|(_vname, _disc, fields)| {
                    fields.iter().map(|(_fn, _off, t)| self.type_ctx.lower(t)).collect()
                })
                .collect();
            union_field_map.insert(ty, vs);
        }
        let mut memo: std::collections::HashMap<TypeId, (u32, u32)> = std::collections::HashMap::new();
        for &(ty, ..) in &lowered {
            if self.type_ctx.interner.is_value_type(ty) {
                let mut in_progress = std::collections::HashSet::new();
                compute_inline_layout(
                    ty,
                    &field_map,
                    &union_field_map,
                    &mut memo,
                    &mut in_progress,
                    &self.type_ctx.interner,
                );
            }
        }
        // Also size every value union (even those not embedded in a value struct), so a value-union
        // local/field/element resolves its full inline footprint via `scalar_size`.
        let value_union_ids: Vec<TypeId> = union_field_map.keys().copied().collect();
        for ty in value_union_ids {
            let mut in_progress = std::collections::HashSet::new();
            compute_inline_layout(
                ty,
                &field_map,
                &union_field_map,
                &mut memo,
                &mut in_progress,
                &self.type_ctx.interner,
            );
        }
        for (ty, sz) in &memo {
            self.type_ctx.interner.set_value_layout(*ty, sz.0, sz.1);
        }
        for (ty, name, defs) in lowered {
            layouts.insert(ty, TypeLayout::from_fields(&self.type_ctx.interner, name, defs));
        }
        for (name, _size, variants) in union_snapshot {
            let ty = self.type_ctx.lower_str(&name);
            let mut vs = Vec::with_capacity(variants.len());
            // Recompute payload offsets with value-aware sizes now that inline value(`struct`)
            // footprints are known: an inline value payload occupies its full size (the declaration
            // pass conservatively sized it as a 4-byte pointer). The discriminant word is at offset 0.
            const DISCRIMINANT_SIZE: u32 = 4;
            let mut block_end = DISCRIMINANT_SIZE;
            for (vname, discriminant, fields) in variants {
                let mut offset = DISCRIMINANT_SIZE;
                let mut flds = Vec::with_capacity(fields.len());
                for (fname, _old_offset, t) in fields {
                    let fty = self.type_ctx.lower(&t);
                    let (fsize, falign) = crate::hir::scalar_size(&self.type_ctx.interner, fty);
                    let rem = offset % falign;
                    if rem != 0 {
                        offset += falign - rem;
                    }
                    flds.push(FieldLayout { offset, ty: fty, name: fname });
                    offset += fsize;
                }
                block_end = block_end.max(offset);
                vs.push(UnionVariant { name: vname, discriminant, fields: flds });
            }
            // Keep the block 8-byte aligned so a `double` payload stays naturally aligned.
            let size = block_end.div_ceil(8) * 8;
            layouts.insert_union(ty, UnionLayout { name, variants: vs, size });
        }
        layouts
    }

    /// Collects the module's host/interop imports: every non-intrinsic `extern fun` (top-level or a
    /// class/`extend` static member) becomes an [`HImport`] the backend emits as `(import ...)`.
    /// Overloaded externs share one imported field, so entries are de-duplicated by name.
    pub(in crate::semantics::analyzer) fn hir_build_imports(
        &mut self,
        node: &crate::syntax::nodes::ProgramNode,
    ) -> Vec<HImport> {
        use crate::types::method_fn;
        let mut imports: Vec<HImport> = Vec::new();
        // Each candidate is paired with the name it was *registered* under: top-level externs keep
        // their bare name, while class/`extend` static externs are mangled `{Type}_{method}` (the
        // name the call site resolves to). Using the bare method name for a class extern would fail
        // the def lookup and silently drop the import (its call site then falls back to `$def{N}`).
        let top = node.functions.iter().map(|f| (f, f.name.text.clone()));
        let class_methods = node
            .structs
            .iter()
            .flat_map(|s| s.methods.iter().map(move |m| (m, method_fn(&s.name.text, &m.name.text))));
        let extend_methods = node
            .extends
            .iter()
            .flat_map(|e| e.methods.iter().map(move |m| (m, method_fn(&e.target.text, &m.name.text))));
        for (func, sym_name) in top.chain(class_methods).chain(extend_methods) {
            if !func.is_extern || crate::intrinsics::has_intrinsic_attr(&func.attributes) {
                continue;
            }
            if imports.iter().any(|i| i.name == sym_name) {
                continue;
            }
            // Match the def the call site resolves to, so the emitter's symbol table maps the call
            // onto this import's `$name`. Unregistered externs (should not happen) are skipped.
            let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, &sym_name) else {
                continue;
            };
            let (module, field) = extern_import_target(func);
            let params = func
                .parameters
                .iter()
                .map(|p| self.type_ctx.lower(&p.type_))
                .collect();
            let ret = match func.return_type.as_ref() {
                Some(t) if *t != Type::Void => Some(self.type_ctx.lower(t)),
                _ => None,
            };
            imports.push(HImport { def, name: sym_name, module, field, params, ret });
        }
        imports
    }

    /// Collects every `@intrinsic("key")` extern as `(callee DefId, key)`. Unlike host imports these
    /// have no `(import ...)` and no emitted body: their call sites resolve directly to the runtime
    /// helper `$<key>` (`string_alloc`, `char_at`, …) or, for `sleep`, are recognized as an async
    /// intrinsic. Methods are looked up under their mangled `{Type}_{method}` def name (the name the
    /// call site resolves to), matching how they were registered.
    pub(in crate::semantics::analyzer) fn hir_build_intrinsics(
        &mut self,
        node: &crate::syntax::nodes::ProgramNode,
    ) -> Vec<(crate::types::DefId, String)> {
        use crate::types::method_fn;
        let mut out: Vec<(crate::types::DefId, String)> = Vec::new();
        for func in node.functions.iter() {
            if let Some(key) = crate::intrinsics::intrinsic_key(&func.attributes) {
                if let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, &func.name.text) {
                    out.push((def, key));
                }
            }
        }
        for s in node.structs.iter() {
            for m in s.methods.iter() {
                if let Some(key) = crate::intrinsics::intrinsic_key(&m.attributes) {
                    let mangled = method_fn(&s.name.text, &m.name.text);
                    if let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, &mangled) {
                        out.push((def, key));
                    }
                }
            }
        }
        for e in node.extends.iter() {
            for m in e.methods.iter() {
                if let Some(key) = crate::intrinsics::intrinsic_key(&m.attributes) {
                    let mangled = method_fn(&e.target.text, &m.name.text);
                    if let Some(def) = self.type_ctx.defs.lookup(DefKind::Function, &mangled) {
                        out.push((def, key));
                    }
                }
            }
        }
        out
    }
}

type FieldMap = std::collections::HashMap<crate::types::TypeId, Vec<crate::types::TypeId>>;
type UnionFieldMap =
    std::collections::HashMap<crate::types::TypeId, Vec<Vec<crate::types::TypeId>>>;
type LayoutMemo = std::collections::HashMap<crate::types::TypeId, (u32, u32)>;

/// Recursively computes the inline `(size, align)` of a value type `ty` — a value (`struct`) type or
/// a value union — memoizing results. A value-typed field contributes its own inline footprint
/// (recursing); every other field is a scalar or a 4-byte reference pointer. A value union is sized
/// as `discriminant(4) + max value-aware variant payload`. `in_progress` guards against value-type
/// cycles (rejected as a semantic error before codegen); a back-edge resolves to a 4-byte
/// placeholder so this computation always terminates.
fn compute_inline_layout(
    ty: crate::types::TypeId,
    field_map: &FieldMap,
    union_field_map: &UnionFieldMap,
    memo: &mut LayoutMemo,
    in_progress: &mut std::collections::HashSet<crate::types::TypeId>,
    interner: &crate::types::TypeInterner,
) -> (u32, u32) {
    if let Some(&sz) = memo.get(&ty) {
        return sz;
    }
    if !in_progress.insert(ty) {
        // Cyclic value type: broken here so sizing terminates (the cycle is a reported error).
        return (4, 4);
    }
    let result = if let Some(variants) = union_field_map.get(&ty) {
        // Value union: discriminant word at offset 0, each variant's payload packed after it; the
        // block is sized to the largest variant and aligned to the widest field.
        const DISCRIMINANT: u32 = 4;
        let mut max_end = DISCRIMINANT;
        let mut max_align = 4u32;
        for fields in variants {
            let mut offset = DISCRIMINANT;
            for &fty in fields {
                let (size, align) =
                    value_field_size(fty, field_map, union_field_map, memo, in_progress, interner);
                let rem = offset % align;
                if rem != 0 {
                    offset += align - rem;
                }
                offset += size;
                max_align = max_align.max(align);
            }
            max_end = max_end.max(offset);
        }
        let rem = max_end % max_align;
        if rem != 0 {
            max_end += max_align - rem;
        }
        (max_end, max_align)
    } else {
        let mut offset = 0u32;
        let mut max_align = 4u32;
        if let Some(fields) = field_map.get(&ty) {
            for &fty in fields {
                let (size, align) =
                    value_field_size(fty, field_map, union_field_map, memo, in_progress, interner);
                let rem = offset % align;
                if rem != 0 {
                    offset += align - rem;
                }
                offset += size;
                max_align = max_align.max(align);
            }
        }
        let rem = offset % max_align;
        if rem != 0 {
            offset += max_align - rem;
        }
        (offset, max_align)
    };
    in_progress.remove(&ty);
    memo.insert(ty, result);
    result
}

/// The inline `(size, align)` of a single field type: a nested value struct/union recurses; a scalar
/// uses its width; any reference is a 4-byte pointer.
fn value_field_size(
    fty: crate::types::TypeId,
    field_map: &FieldMap,
    union_field_map: &UnionFieldMap,
    memo: &mut LayoutMemo,
    in_progress: &mut std::collections::HashSet<crate::types::TypeId>,
    interner: &crate::types::TypeInterner,
) -> (u32, u32) {
    use crate::types::{PrimTy, TyKind};
    let stripped = interner.strip_nullable(fty);
    if interner.is_value_type(stripped) {
        return compute_inline_layout(stripped, field_map, union_field_map, memo, in_progress, interner);
    }
    match interner.kind(stripped) {
        TyKind::Prim(PrimTy::Bool | PrimTy::Char | PrimTy::Byte) => (1, 1),
        TyKind::Prim(PrimTy::Double | PrimTy::Long | PrimTy::ULong) => (8, 8),
        _ => (4, 4),
    }
}
