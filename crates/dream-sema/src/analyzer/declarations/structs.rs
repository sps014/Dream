//! Struct/class registration: field layout, value-vs-reference classification, value-containment
//! soundness checks, and generic-struct instantiation.

use super::*;
use dream_syntax::nodes::struct_node::{StructDeclarationNode, StructFieldNode};
use dream_syntax::nodes::types::mangle_generic;

impl<'a> Analyzer<'a> {
    /// Pass 0: register every (non-generic) struct and its methods; stash generic templates.
    pub(in crate::analyzer) fn register_structs(
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
            if struct_decl.is_ref_struct {
                self.type_ctx.interner.mark_ref_struct_def(def);
            }
            if struct_decl
                .attributes
                .iter()
                .any(|a| a.name.text == "shared")
            {
                self.type_ctx.interner.mark_shared_def(def);
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
        }

        // `@shared class` closed-graph rule: every field must be safe to access from another
        // thread without going through this class's own lock — either unmanaged/value-typed
        // (copied, no shared heap pointer) or itself another `@shared` type (guarded by its own
        // lock). Run once every non-generic class's `is_shared`/fields are registered above, so a
        // field referencing another `@shared` class declared later in the same file still resolves.
        for struct_decl in node.structs.iter() {
            if struct_decl.generic_parameters.is_some() {
                continue;
            }
            if !struct_decl
                .attributes
                .iter()
                .any(|a| a.name.text == "shared")
            {
                continue;
            }
            for field in &struct_decl.fields {
                self.check_shared_field(&struct_decl.name.text, field, diagnostics);
            }
        }

        // `weak`/`unowned` field validation and the whole-program class reference-cycle check run
        // last, once every non-generic class's fields are in `self.struct_table` (needed to
        // classify a field's target as a value struct vs. a class).
        self.check_weak_unowned_and_cycles(node, diagnostics);

        // A `ref struct` field would smuggle a stack-only value into a heap-allocated (or
        // otherwise longer-lived) container — reject it regardless of whether the enclosing type
        // is a `class` or a `struct`. Run once every struct's own `is_ref_struct`/`is_value` marks
        // are registered (the loop above), so a field referencing another `ref struct` declared
        // later in the same file still resolves correctly.
        for struct_decl in node.structs.iter() {
            for field in &struct_decl.fields {
                self.reject_ref_struct_field(&struct_decl.name.text, field, diagnostics);
            }
        }
    }

    /// Reports an error unless `field`'s type is safe to store in an `@shared class`: unmanaged
    /// (blittable, copied by value — a primitive or a value `struct` with no reference fields) or
    /// itself another `@shared` type (guarded by its own embedded lock). A plain (non-`@shared`)
    /// class/array/string field would let a second thread reach mutable, unlocked heap state
    /// through this field without going through *any* lock.
    fn check_shared_field(
        &mut self,
        owner_name: &str,
        field: &StructFieldNode,
        diagnostics: &mut DiagnosticBag,
    ) {
        if self.type_satisfies_kind(
            &field.field_type,
            dream_syntax::nodes::ConstraintKind::Unmanaged,
        ) {
            return;
        }
        let field_type_name = field.field_type.get_type();
        if let Some(def) = self
            .type_ctx
            .defs
            .lookup(DefKind::Struct, field_type_name.as_str())
        {
            if self.type_ctx.interner.is_shared_def(def) {
                return;
            }
        }
        diagnostics.report_error(
            format!(
                "field '{}' of '@shared class {}' has type '{}', which is neither unmanaged nor itself '@shared': an '@shared class' may only hold unmanaged/value fields or references to other '@shared' types",
                field.name.text,
                owner_name,
                field_type_name
            ),
            Some(field.name.position),
        );
    }

    /// Reports an error if `field`'s type is a `ref struct` — such a type cannot be stored as a
    /// field of any enclosing type (`class` or `struct`), since that would let a stack-only value
    /// outlive the stack frame it was created in.
    pub(in crate::analyzer) fn reject_ref_struct_field(
        &mut self,
        owner_name: &str,
        field: &StructFieldNode,
        diagnostics: &mut DiagnosticBag,
    ) {
        let tid = self.type_ctx.lower(&field.field_type);
        if self.type_ctx.interner.is_ref_struct_type(tid) {
            diagnostics.report_error(
                format!(
                    "field '{}' of '{}' cannot have type '{}': a 'ref struct' cannot be stored as a field (it would let a stack-only value escape its stack frame)",
                    field.name.text,
                    owner_name,
                    field.field_type.get_type()
                ),
                Some(field.name.position),
            );
        }
    }

    /// Rejects a `ref struct`-typed parameter on any `async` function, method, or `extend`-block
    /// method in the program: an `async` call may suspend at an `await`, which spills the coroutine's
    /// live locals (including its parameters) into a heap-allocated state object so they survive
    /// across the suspend point — exactly the kind of stack-frame escape a `ref struct` forbids.
    /// Generic templates are checked once per instantiation's concrete parameter types would be
    /// ideal, but templates don't carry a `ref struct` argument until monomorphized, so this walks
    /// only concrete (non-generic) declarations, matching this analysis's stated conservative scope.
    pub(in crate::analyzer) fn check_ref_struct_async_boundary(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        let check_fn = |this: &mut Self,
                        f: &dream_syntax::nodes::function::FunctionNode<'a>,
                        diags: &mut DiagnosticBag| {
            if !f.is_async {
                return;
            }
            for p in &f.parameters {
                let tid = this.type_ctx.lower(&p.type_);
                if this.type_ctx.interner.is_ref_struct_type(tid) {
                    diags.report_error(
                        format!(
                            "async function '{}' cannot take 'ref struct' parameter '{}' of type '{}': it may need to survive an 'await' suspend point, which would spill it into the heap-allocated coroutine state",
                            f.name.text,
                            p.name.text,
                            p.type_.get_type()
                        ),
                        Some(f.name.position),
                    );
                }
            }
        };
        for f in node.functions.iter() {
            check_fn(self, f, diagnostics);
        }
        for s in node.structs.iter() {
            for m in &s.methods {
                check_fn(self, m, diagnostics);
            }
        }
        for e in node.extends.iter() {
            for m in &e.methods {
                check_fn(self, m, diagnostics);
            }
        }
        for en in node.enums.iter() {
            for m in &en.methods {
                check_fn(self, m, diagnostics);
            }
        }
    }

    /// Rejects any `ref struct` type appearing in `args` as a generic type argument: instantiating
    /// a generic class/struct/union/function with a `ref struct` argument would store it in a field,
    /// array element, or heap payload somewhere in that generic's body, letting a stack-only value
    /// escape its frame. Called at every generic instantiation site (classes, unions, and — where
    /// wired — generic function calls).
    pub(in crate::analyzer) fn reject_ref_struct_type_args(
        &mut self,
        args: &[Type],
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        for arg in args {
            let tid = self.type_ctx.lower(arg);
            if self.type_ctx.interner.is_ref_struct_type(tid) {
                diagnostics.report_error(
                    format!(
                        "'{}' is a 'ref struct' and cannot be used as a generic type argument (it would be stored in a heap-allocated container, letting it escape its stack frame)",
                        arg.get_type()
                    ),
                    Some(*position),
                );
            }
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
    /// the value-containment graph). Array fields are references.
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
            let base = type_name.as_str();
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

    pub(in crate::analyzer) fn ensure_struct_instantiated(
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
        self.generic_struct_instances
            .push((base_name.to_string(), args.to_vec()));

        let params = template.generic_parameters.as_deref().unwrap_or(&[]);
        Self::check_generic_arity(
            "class",
            base_name,
            params.len(),
            args.len(),
            position,
            diagnostics,
        );
        self.reject_ref_struct_type_args(args, position, diagnostics);
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
                visibility: field.visibility,
                is_weak: field.is_weak,
                is_unowned: field.is_unowned,
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
            template.visibility,
        );
        new_decl.is_value = template.is_value;
        new_decl.is_ref_struct = template.is_ref_struct;
        new_decl.file_path = template.file_path.clone();

        let new_decl_ref: &'a StructDeclarationNode<'a> = self.arena.alloc(new_decl);

        if let Err(e) = self.struct_table.add_struct(new_decl_ref) {
            diagnostics.report_error(e, Some(*position));
        }

        // Value-struct soundness is checked per instantiation (the template's fields are generic, so
        // whether this monomorphization embeds itself by value is only decidable once `T` is
        // concrete).
        if new_decl_ref.is_value && self.value_struct_contains_self(&mangled_name) {
            diagnostics.report_error(
                    format!(
                        "value struct '{}' cannot contain itself by value; use a reference type ('class') or an array to break the cycle",
                        mangled_name
                    ),
                    Some(*position),
                );
        }

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
