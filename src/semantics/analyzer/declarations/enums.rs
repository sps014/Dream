//! C-style integer enums and discriminated unions: registration, variant layout/discriminants,
//! value-vs-heap union classification, generic-union instantiation, and generic `extend`-block
//! method attachment.

use super::*;
use crate::semantics::union_table::{
    UnionFieldInfo, UnionInfo, UnionVariantInfo, DISCRIMINANT_SIZE,
};
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::EnumVariantNode;
use crate::types::value_size_align;

impl<'a> Analyzer<'a> {
    /// Pass: register every enum. A C-style integer enum (no payloads) goes into the enum table
    /// (member -> integer value). A discriminated union (any variant carries a payload) is
    /// registered as a heap reference type with a computed layout; generic unions are stashed as
    /// templates and instantiated on demand. Reports duplicate enum/member names.
    pub(in crate::semantics::analyzer) fn register_enums(
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
                (enum_decl.file_path.clone(), enum_decl.is_public),
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
        }

        // Pass 2: register concrete (non-generic) discriminated unions. Their payload fields may
        // instantiate generic unions whose templates were collected in pass 1.
        for enum_decl in node.enums.iter() {
            if enum_decl.is_data_enum() && enum_decl.generic_parameters.is_none() {
                self.register_union(
                    &enum_decl.name.text,
                    &enum_decl.variants,
                    &GenericBindings::new(),
                    diagnostics,
                );
            }
        }
    }

    /// Computes and registers the layout of a (possibly monomorphized) discriminated union under
    /// `union_name`. Each variant's payload starts after the discriminant word; payloads of
    /// different variants overlap, so the block is sized to the largest variant. `bindings`
    /// substitutes any generic parameters in field types (empty for non-generic unions).
    pub(in crate::semantics::analyzer) fn register_union(
        &mut self,
        union_name: &str,
        variants: &[EnumVariantNode],
        bindings: &GenericBindings,
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
        if let Err(e) = self.struct_table.add_union(union_name, size, true, None) {
            diagnostics.report_error(e, None);
            return;
        }

        // A data enum instance becomes a *value* union (stored inline, copy semantics, no heap
        // allocation) when every variant payload is itself value/primitive. Decided here, per
        // (monomorphized) instance, because `Option<int>` (value) and `Option<string>` (heap) share
        // one `DefId`. The inline layout is finalized later in `hir_build_layouts` (value-aware sizes).
        let all_value = variant_infos.iter().all(|v| {
            v.fields
                .iter()
                .all(|f| self.payload_type_is_value(&f.type_))
        });
        if all_value {
            let union_tid = self.type_ctx.lower_str(union_name);
            self.type_ctx.interner.mark_value_union(union_tid);
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
        let stripped = self.type_ctx.interner.strip_nullable(tid);
        if self.type_ctx.interner.is_value_type(stripped) {
            return true;
        }
        matches!(
            self.type_ctx.interner.kind(stripped),
            crate::types::TyKind::Prim(p) if *p != crate::types::PrimTy::String
        )
    }

    /// Ensures a generic union instantiation (e.g. `Option<int>` -> `Option_int`) is registered,
    /// monomorphizing its variant field types. No-op for non-generic or already-registered unions.
    pub(in crate::semantics::analyzer) fn ensure_union_instantiated(
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
        let bindings = generic_bindings(params, args);
        self.register_union(&mangled, &template.variants, &bindings, diagnostics);
        self.register_generic_extension_methods(base_name, &mangled, args, diagnostics);
    }

    /// If a generic `extend` block targets `base_name` (e.g. `extend Option<T> { ... }`),
    /// monomorphizes its methods for the concrete instantiation `mangled` (e.g. `Option_int`),
    /// binding the extend block's own generic parameters to `args` in declaration order. A no-op
    /// when no generic extension targets `base_name`.
    pub(in crate::semantics::analyzer) fn register_generic_extension_methods(
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

    /// True when every generic constraint on an `extend` block is satisfied by the concrete
    /// bindings of one instantiation. Unlike class/function constraints, an unsatisfied extension
    /// constraint is not an error — the extension's methods simply do not attach to that instance.
    fn extension_constraints_satisfied(
        &self,
        constraints: &[crate::syntax::nodes::GenericConstraint],
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
    pub(in crate::semantics::analyzer) fn ensure_type_instantiated(
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
    pub(in crate::semantics::analyzer) fn enum_member_value(
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
