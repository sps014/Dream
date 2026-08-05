//! C-style integer enums and discriminated unions: registration, variant layout/discriminants,
//! value-vs-heap union classification, generic-union instantiation, and generic `extend`-block
//! method attachment.

use super::*;
use crate::union_table::{
    UnionFieldInfo, UnionInfo, UnionVariantInfo, DISCRIMINANT_SIZE,
};
use dream_syntax::nodes::types::mangle_generic;
use dream_syntax::nodes::EnumVariantNode;
use dream_types::value_size_align;

impl<'a> Analyzer<'a> {
    /// Pass: register every enum. A C-style integer enum (no payloads) goes into the enum table
    /// (member -> integer value). A discriminated union (any variant carries a payload) is
    /// registered as a heap reference type with a computed layout; generic unions are stashed as
    /// templates and instantiated on demand. Reports duplicate enum/member names.
    pub(in crate::analyzer) fn register_enums(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        // Pass 1: register C-style enums and stash generic-union *templates*. Doing templates
        // first means a concrete union may reference a generic union declared later (or one from
        // the prelude, which is merged after user code), e.g. `enum Pair { Both(Option<int>) }`.
        for enum_decl in node.enums.iter() {
            let name = &enum_decl.name.text;
            if enum_decl.is_sealed {
                self.sealed_types.insert(name.clone());
            }
            self.type_visibility.insert(
                name.clone(),
                (enum_decl.file_path.clone(), enum_decl.visibility),
            );
            if self.enum_table.contains_key(name)
                || self.union_table.contains_key(name)
                || self.generic_unions.contains_key(name)
            {
                diagnostics.report_error(
                    format!("Enum '{}' is already defined", name),
                    Some(enum_decl.name.position),
                );
                continue;
            }

            if enum_decl.is_data_enum() {
                // Generic discriminated unions are templates, monomorphized on first use.
                if enum_decl.generic_parameters.is_some() {
                    self.type_ctx.register(
                        DefKind::Union,
                        name,
                        generic_param_names(&enum_decl.generic_parameters),
                    );
                    self.generic_unions.insert(name.clone(), enum_decl);
                }
                continue;
            }

            // C-style integer enum: members lower to plain `i32` constants. Insertion-ordered so
            // codegen interns the variant names deterministically.
            let mut members = indexmap::IndexMap::new();
            for variant in enum_decl.variants.iter() {
                if members.contains_key(&variant.name.text) {
                    diagnostics.report_error(
                        format!(
                            "Duplicate member '{}' in enum '{}'",
                            variant.name.text, name
                        ),
                        Some(variant.name.position),
                    );
                    continue;
                }
                members.insert(variant.name.text.clone(), variant.value);
            }
            self.type_ctx.register(DefKind::Enum, name, vec![]);
            self.enum_table.insert(name.clone(), members);
            // C-style enums may declare methods in the body (same as unions/classes).
            if !enum_decl.methods.is_empty() {
                self.register_methods_for(
                    name,
                    &enum_decl.methods,
                    &GenericBindings::new(),
                    diagnostics,
                );
            }
        }

        // Pass 2: register concrete (non-generic) discriminated unions. Their payload fields may
        // instantiate generic unions whose templates were collected in pass 1.
        for enum_decl in node.enums.iter() {
            if enum_decl.is_data_enum() && enum_decl.generic_parameters.is_none() {
                let has_stack = enum_decl.attributes.iter().any(|a| a.name.text == "stack");
                self.register_union(
                    &enum_decl.name.text,
                    &enum_decl.variants,
                    &GenericBindings::new(),
                    has_stack,
                    diagnostics,
                );
                if !enum_decl.methods.is_empty() {
                    self.register_methods_for(
                        &enum_decl.name.text,
                        &enum_decl.methods,
                        &GenericBindings::new(),
                        diagnostics,
                    );
                }
            }
        }
    }

    /// Computes and registers the layout of a (possibly monomorphized) discriminated union under
    /// `union_name`. Each variant's payload starts after the discriminant word; payloads of
    /// different variants overlap, so the block is sized to the largest variant. `bindings`
    /// substitutes any generic parameters in field types (empty for non-generic unions).
    /// `has_stack` is true when the union declaration carries `@stack`: a checked contract that
    /// every monomorphized instance must qualify as inline/value (all-value payloads, or any
    /// number of non-self-referential reference payloads — self-reference is rejected), rather
    /// than just the usual best-effort automatic classification.
    pub(in crate::analyzer) fn register_union(
        &mut self,
        union_name: &str,
        variants: &[EnumVariantNode],
        bindings: &GenericBindings,
        has_stack: bool,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mut variant_infos = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut block_end = DISCRIMINANT_SIZE;

        for variant in variants {
            if !seen.insert(variant.name.text.clone()) {
                diagnostics.report_error(
                    format!(
                        "Duplicate variant '{}' in enum '{}'",
                        variant.name.text, union_name
                    ),
                    Some(variant.name.position),
                );
                continue;
            }
            let mut offset = DISCRIMINANT_SIZE;
            let mut field_infos = Vec::new();
            for field in &variant.fields {
                let ftype = substitute_generic_type(&field.field_type, bindings);
                // Instantiate any generic union/struct referenced by a payload field type.
                if let Some((base, args)) = Self::resolve_struct_parts(&ftype) {
                    if !args.is_empty() {
                        self.ensure_type_instantiated(
                            &base,
                            &args,
                            &field.name.position,
                            diagnostics,
                        );
                    }
                }
                let ftid = self.type_ctx.lower(&ftype);
                if self.type_ctx.interner.is_ref_struct_type(ftid) {
                    diagnostics.report_error(
                        format!(
                            "field '{}' of variant '{}' cannot have type '{}': a 'ref struct' cannot be stored as a union payload (it would let a stack-only value escape its stack frame)",
                            field.name.text,
                            variant.name.text,
                            ftype.get_type()
                        ),
                        Some(field.name.position),
                    );
                }
                let (size, align) = value_size_align(&ftype.get_type());
                let rem = offset % align;
                if rem != 0 {
                    offset += align - rem;
                }
                field_infos.push(UnionFieldInfo {
                    name: field.name.text.clone(),
                    type_: ftype,
                    offset,
                });
                offset += size;
            }
            block_end = block_end.max(offset);
            variant_infos.push(UnionVariantInfo {
                name: variant.name.text.clone(),
                discriminant: variant.value,
                fields: field_infos,
            });
        }

        // Align the block to 8 bytes so a `double` payload stays naturally aligned.
        let size = block_end.div_ceil(8) * 8;

        self.type_ctx.register(DefKind::Union, union_name, vec![]);
        // Data-enum unions are treated as always visible here; C-style enum visibility is tracked
        // separately in `enum_visibility` and checked at type-reference sites.
        if let Err(e) = self.struct_table.add_union(
            union_name,
            size,
            dream_syntax::nodes::Visibility::Public,
            None,
        ) {
            diagnostics.report_error(e, None);
            return;
        }

        // A data enum instance becomes a *value* union (stored inline, copy semantics, no heap
        // allocation) when every variant payload is itself value/primitive. Decided here, per
        // (monomorphized) instance, because `Option<int>` (value) and `Option<string>` (heap) share
        // one `DefId`. The inline layout is finalized later in `hir_build_layouts` (value-aware sizes).
        //
        // `@stack` additionally allows any number of reference-typed payload fields to still go
        // inline, each stored as a retained pointer exactly like a reference field embedded in a
        // value `struct` already is (see `construct_value_union` in
        // `mir::emit::emitter::value_struct`). Self-reference is still rejected: an inline recursive
        // value union would have infinite size. This relaxation is opt-in only, gated behind
        // `@stack`, rather than automatic: several existing constructs (e.g. a `weak parent:
        // Option<Node>` field, see `docs/language/memory.md`) depend on `Option<T>` staying a heap
        // reference whenever `T` is itself a reference type, so widening the automatic inference
        // would silently change their runtime representation. `@stack` is an explicit, checked
        // opt-in instead: it errors if the union is self-referential (rather than silently
        // falling back to the heap), and its scope is limited to declarations that ask for it.
        let union_tid = self.type_ctx.lower_str(union_name);
        let mut ref_count = 0usize;
        let mut self_ref_field: Option<(String, String)> = None;
        for v in &variant_infos {
            for f in &v.fields {
                if self.payload_type_is_value(&f.type_) {
                    continue;
                }
                ref_count += 1;
                if self.type_ctx.lower(&f.type_) == union_tid && self_ref_field.is_none() {
                    self_ref_field = Some((f.name.clone(), f.type_.get_type()));
                }
            }
        }
        let self_referential = self_ref_field.is_some();
        let all_value = ref_count == 0;
        let stack_inlineable = has_stack && !self_referential;
        if all_value || stack_inlineable {
            self.type_ctx.interner.mark_value_union(union_tid);
        } else if has_stack {
            let (field_name, field_type) = self_ref_field
                .unwrap_or_else(|| ("<variant>".to_string(), union_name.to_string()));
            diagnostics.report_error(
                format!(
                    "'@stack' union '{}' cannot be stored inline: field '{}' has type '{}', which is a self-referential payload that would make this value type infinite-size",
                    union_name, field_name, field_type,
                ),
                None,
            );
        }

        self.union_table.insert(
            union_name.to_string(),
            UnionInfo {
                name: union_name.to_string(),
                variants: variant_infos,
                size,
            },
        );
    }

    /// True when a union payload field of type `ty` is stored by value: a non-string primitive, a
    /// value (`struct`) type, or an already-registered value union. Strings, classes, arrays, and
    /// heap unions are references (which force the enclosing union onto the heap).
    fn payload_type_is_value(&mut self, ty: &Type) -> bool {
        let tid = self.type_ctx.lower(ty);
        if self.type_ctx.interner.is_value_type(tid) {
            return true;
        }
        matches!(
            self.type_ctx.interner.kind(tid),
            dream_types::TyKind::Prim(p) if *p != dream_types::PrimTy::String
        )
    }

    /// Ensures a generic union instantiation (e.g. `Option<int>` -> `Option_int`) is registered,
    /// monomorphizing its variant field types. No-op for non-generic or already-registered unions.
    pub(in crate::analyzer) fn ensure_union_instantiated(
        &mut self,
        base_name: &str,
        args: &[Type],
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mangled = mangle_generic(base_name, args);
        self.type_ctx
            .register_instance(DefKind::Union, base_name, args);
        if self.union_table.contains_key(&mangled) {
            return;
        }
        let template = match self.generic_unions.get(base_name) {
            Some(t) => *t,
            None => return,
        };
        let params = template.generic_parameters.as_deref().unwrap_or(&[]);
        Self::check_generic_arity(
            "enum",
            base_name,
            params.len(),
            args.len(),
            position,
            diagnostics,
        );
        self.reject_ref_struct_type_args(args, position, diagnostics);
        let bindings = generic_bindings(params, args);
        let has_stack = template.attributes.iter().any(|a| a.name.text == "stack");
        self.register_union(
            &mangled,
            &template.variants,
            &bindings,
            has_stack,
            diagnostics,
        );
        // Methods declared on the generic enum template (e.g. `Option.is_some`) attach to each
        // monomorphization the same way class methods do.
        if !template.methods.is_empty() {
            self.verify_generic_constraints(
                &template.generic_constraints,
                &bindings,
                position,
                diagnostics,
            );
            self.register_methods_for(&mangled, &template.methods, &bindings, diagnostics);
        }
        self.register_generic_extension_methods(base_name, &mangled, args, diagnostics);
    }

    /// If a generic `extend` block targets `base_name` (e.g. `extend Option<T> { ... }`),
    /// monomorphizes its methods for the concrete instantiation `mangled` (e.g. `Option_int`),
    /// binding the extend block's own generic parameters to `args` in declaration order. A no-op
    /// when no generic extension targets `base_name`.
    pub(in crate::analyzer) fn register_generic_extension_methods(
        &mut self,
        base_name: &str,
        mangled: &str,
        args: &[Type],
        diagnostics: &mut DiagnosticBag,
    ) {
        let exts: Vec<&'a ExtendNode<'a>> = match self.generic_extends.get(base_name) {
            Some(list) => list.clone(),
            None => return,
        };
        for ext in exts {
            let ext_params = ext.generic_parameters.as_deref().unwrap_or(&[]);
            let ext_bindings = generic_bindings(ext_params, args);
            // A constrained extension (`extend List<T : Comparable<T>>`) only applies to instances
            // whose argument satisfies the bound; skip attaching its methods otherwise (so e.g.
            // `List<int>.sort()` is simply "no such method" unless `int` is made `Comparable`).
            if !self.extension_constraints_satisfied(&ext.generic_constraints, &ext_bindings) {
                continue;
            }
            self.register_methods_for(mangled, &ext.methods, &ext_bindings, diagnostics);
        }
    }

    /// True when every generic constraint on an `extend` block or method `where` clause is
    /// satisfied by the concrete bindings of one instantiation. Unlike class/function constraints,
    /// an unsatisfied attachment constraint is not an error — the methods simply do not attach.
    pub(in crate::analyzer) fn extension_constraints_satisfied(
        &self,
        constraints: &[dream_syntax::nodes::GenericConstraint],
        bindings: &GenericBindings,
    ) -> bool {
        constraints.iter().all(|c| {
            bindings.get(&c.param.text).is_some_and(|concrete| {
                c.bounds
                    .iter()
                    .all(|bound| self.type_satisfies_bound(concrete, bound, bindings))
                    && c.kinds
                        .iter()
                        .all(|kind| self.type_satisfies_kind(concrete, *kind))
            })
        })
    }

    /// Instantiates whichever generic container `base_name` denotes (a generic class or a generic
    /// discriminated union), so nested generic types in field/argument positions are resolved.
    pub(in crate::analyzer) fn ensure_type_instantiated(
        &mut self,
        base_name: &str,
        args: &[Type],
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        if self.generic_unions.contains_key(base_name) {
            self.ensure_union_instantiated(base_name, args, position, diagnostics);
        } else {
            self.ensure_struct_instantiated(base_name, args, position, diagnostics);
        }
    }

    /// Returns the integer value of an enum member, if `enum_name.member` names a known enum member.
    pub(in crate::analyzer) fn enum_member_value(
        &self,
        enum_name: &str,
        member: &str,
    ) -> Option<i32> {
        self.enum_table
            .get(enum_name)
            .and_then(|m| m.get(member))
            .copied()
    }
}
