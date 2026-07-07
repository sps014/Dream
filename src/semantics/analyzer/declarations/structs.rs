//! Struct/class registration: field layout, value-vs-reference classification, value-containment
//! soundness checks, and generic-struct instantiation.

use super::*;
use crate::syntax::nodes::struct_node::{StructDeclarationNode, StructFieldNode};
use crate::syntax::nodes::types::mangle_generic;

impl<'a> Analyzer<'a> {
    /// Pass 0: register every (non-generic) struct and its methods; stash generic templates.
    pub(in crate::semantics::analyzer) fn register_structs(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        for struct_decl in node.structs.iter() {
            diagnostics.file_path = file_path_string(&struct_decl.file_path);
            if struct_decl.is_sealed {
                self.sealed_types.insert(struct_decl.name.text.clone());
            }
            let def = self.type_ctx.register(
                DefKind::Struct,
                &struct_decl.name.text,
                generic_param_names(&struct_decl.generic_parameters),
            );
            // A `struct` is a value type: record it on the def table and the interner so
            // reference-classification (RC, layout, codegen) treats its instances as inline values.
            if struct_decl.is_value {
                self.type_ctx.defs.mark_value(def);
                self.type_ctx.interner.mark_value_def(def);
                // A value struct may implement interfaces (e.g. `Comparable`/`Equatable`): its
                // methods dispatch *statically* through direct calls and generic constraints with no
                // boxing. Widening it to an interface *reference* (or `object`) boxes it into a fresh
                // tagged heap copy at the upcast site — see the value struct case in `emit_cast`.
            }
            if struct_decl.generic_parameters.is_some() {
                // A generic class may implement a (generic or non-generic) interface; the
                // `implements` clause is validated per monomorphization in `ensure_struct_instantiated`.
                // Async methods are supported: each monomorphization registers the method as a
                // distinct concrete function (see `register_struct_methods`), so its async state
                // machine is generated per instance like any other async method.
                self.generic_structs
                    .insert(struct_decl.name.text.clone(), struct_decl);
                continue;
            }
            if let Err(e) = self.struct_table.add_struct(struct_decl) {
                diagnostics.report_error(e, Some(struct_decl.name.position));
            }
            self.register_struct_methods(
                struct_decl,
                &struct_decl.name.text,
                &GenericBindings::new(),
                diagnostics,
            );
            self.validate_implements(
                &struct_decl.name.text,
                &struct_decl.implements,
                &struct_decl.methods,
                &GenericBindings::new(),
                struct_decl.name.position,
                diagnostics,
            );
        }

        // A value (`struct`) type is stored inline, so it cannot (transitively) contain itself by
        // value — that would require infinite storage. A reference (`class`) or array field breaks
        // the cycle. Generic value structs are checked per instantiation.
        for struct_decl in node.structs.iter() {
            if struct_decl.generic_parameters.is_some() {
                continue;
            }
            let name = &struct_decl.name.text;
            let is_value = self
                .struct_table
                .get_struct(name)
                .map(|s| s.is_value)
                .unwrap_or(false);
            if is_value && self.value_struct_contains_self(name) {
                diagnostics.report_error(
                    format!(
                        "value struct '{}' cannot contain itself by value; use a reference type ('class') or an array to break the cycle",
                        name
                    ),
                    Some(struct_decl.name.position),
                );
            }
            // A nullable value struct field (`T?`) is stored as a nullable heap pointer to a boxed
            // copy of `T` (see `is_nullable_boxed_value`), so `null` is representable. No rejection.
        }
    }

    /// True when value struct `start` transitively embeds itself by value. Only value-typed,
    /// non-array fields form inline edges; reference fields (`class`, `string`, arrays) do not.
    fn value_struct_contains_self(&self, start: &str) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut work = self.value_struct_field_targets(start);
        while let Some(cur) = work.pop() {
            if cur == start {
                return true;
            }
            if !visited.insert(cur.clone()) {
                continue;
            }
            work.extend(self.value_struct_field_targets(&cur));
        }
        false
    }

    /// The names of value-struct types embedded *by value* in `name`'s fields (the inline edges of
    /// the value-containment graph). Nullable suffixes are stripped; array fields are references.
    fn value_struct_field_targets(&self, name: &str) -> Vec<String> {
        let Some(info) = self.struct_table.get_struct(name) else {
            return Vec::new();
        };
        if !info.is_value {
            return Vec::new();
        }
        let mut out = Vec::new();
        for f in info.fields.values() {
            let type_name = f.type_.get_type();
            let base = type_name.trim_end_matches('?');
            if base.ends_with("[]") {
                continue;
            }
            if let Some(field_info) = self.struct_table.get_struct(base) {
                if field_info.is_value {
                    out.push(base.to_string());
                }
            }
        }
        out
    }

    pub(in crate::semantics::analyzer) fn ensure_struct_instantiated(
        &mut self,
        base_name: &str,
        args: &[Type],
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mangled_name = mangle_generic(base_name, args);
        // Canonicalize the mangled bare name to the structured `(base def, args)` id so both
        // spellings of this instance lower identically.
        self.type_ctx
            .register_instance(DefKind::Struct, base_name, args);
        if self.struct_table.get_struct(&mangled_name).is_some() {
            return;
        }

        let template = match self.generic_structs.get(base_name) {
            Some(template) => *template,
            None => return,
        };

        let params = template.generic_parameters.as_deref().unwrap_or(&[]);
        Self::check_generic_arity(
            "class",
            base_name,
            params.len(),
            args.len(),
            position,
            diagnostics,
        );
        let bindings = generic_bindings(params, args);

        // A constrained class/struct parameter (`class Sorted<T : Comparable<T>>`) must be satisfied
        // by the concrete argument at this instantiation.
        self.verify_generic_constraints(
            &template.generic_constraints,
            &bindings,
            position,
            diagnostics,
        );

        let new_fields: Vec<StructFieldNode> = template
            .fields
            .iter()
            .map(|field| StructFieldNode {
                attributes: field.attributes.clone(),
                name: field.name.clone(),
                is_public: field.is_public,
                type_token: substitute_generic_token(&field.type_token, &bindings),
                field_type: substitute_generic_type(&field.field_type, &bindings),
            })
            .collect();

        let mut new_name_token = template.name.clone();
        new_name_token.text = mangled_name.clone();
        let mut new_decl = StructDeclarationNode::new(
            template.attributes.clone(),
            new_name_token,
            None,
            new_fields,
            template.methods.clone(),
            template.is_public,
        );
        new_decl.is_value = template.is_value;
        new_decl.file_path = template.file_path.clone();

        let new_decl_ref: &'a StructDeclarationNode<'a> = self.arena.alloc(new_decl);

        if let Err(e) = self.struct_table.add_struct(new_decl_ref) {
            diagnostics.report_error(e, Some(*position));
        }

        // Value-struct soundness is checked per instantiation (the template's fields are generic, so
        // whether this monomorphization embeds itself by value or carries a nullable value field is
        // only decidable once `T` is concrete).
        if new_decl_ref.is_value && self.value_struct_contains_self(&mangled_name) {
            diagnostics.report_error(
                    format!(
                        "value struct '{}' cannot contain itself by value; use a reference type ('class') or an array to break the cycle",
                        mangled_name
                    ),
                    Some(*position),
                );
        }
        // A nullable value struct field (`T?`) boxes to a nullable heap pointer, so `null` is
        // representable — no rejection (see the non-generic path above).

        self.register_struct_methods(new_decl_ref, &mangled_name, &bindings, diagnostics);
        self.register_generic_extension_methods(base_name, &mangled_name, args, diagnostics);

        // Validate this monomorphization's `implements` clause: substitute the class type parameters
        // through each listed interface (`Container<T>` -> `Container<int>`) and match the (also
        // substituted) method signatures. Records `implements[Box_int] = [Container_int]`.
        if !template.implements.is_empty() {
            let sub_impls: Vec<Type> = template
                .implements
                .iter()
                .map(|t| substitute_generic_type(t, &bindings))
                .collect();
            self.validate_implements(
                &mangled_name,
                &sub_impls,
                &template.methods,
                &bindings,
                *position,
                diagnostics,
            );
        }
    }
}
