//! Top-level declaration registration: the analyzer passes that populate the type/symbol tables
//! before bodies are checked. Covers structs (fields, layout, value-vs-reference classification),
//! enums, discriminated unions (variants + discriminants), globals, and generic-struct
//! instantiation (`ensure_*_instantiated`). Interface registration lives in the sibling
//! `register_interfaces` module and method/`extend` registration in `register_methods`; both are
//! `impl Analyzer` blocks split out of this file to keep each focused.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::FunctionTableInfo;
use crate::semantics::symbol_table::SymbolTable;
use crate::semantics::union_table::{
    UnionFieldInfo, UnionInfo, UnionVariantInfo, DISCRIMINANT_SIZE,
};
use crate::syntax::nodes::struct_node::{StructDeclarationNode, StructFieldNode};
use crate::syntax::nodes::types::{mangle_generic, strip_array, strip_nullable};
use crate::syntax::nodes::{EnumVariantNode, FunctionNode, ProgramNode, Type};
use crate::syntax::token::token_kind::TokenKind;
use crate::text::text_span::TextSpan;
use crate::types::value_size_align;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Pass: register every enum. A C-style integer enum (no payloads) goes into the enum table
    /// (member -> integer value). A discriminated union (any variant carries a payload) is
    /// registered as a heap reference type with a computed layout; generic unions are stashed as
    /// templates and instantiated on demand. Reports duplicate enum/member names.
    pub(super) fn register_enums(
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
    pub(super) fn register_union(
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
    pub(super) fn ensure_union_instantiated(
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
        if args.len() != params.len() {
            diagnostics.report_error(
                format!(
                    "Generic enum '{}' expects {} type argument(s), but {} were provided",
                    base_name,
                    params.len(),
                    args.len()
                ),
                Some(*position),
            );
        }
        let bindings = generic_bindings(params, args);
        self.register_union(&mangled, &template.variants, &bindings, diagnostics);
        self.register_generic_extension_methods(base_name, &mangled, args, diagnostics);
    }

    /// If a generic `extend` block targets `base_name` (e.g. `extend Option<T> { ... }`),
    /// monomorphizes its methods for the concrete instantiation `mangled` (e.g. `Option_int`),
    /// binding the extend block's own generic parameters to `args` in declaration order. A no-op
    /// when no generic extension targets `base_name`.
    pub(super) fn register_generic_extension_methods(
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
            })
        })
    }

    /// Instantiates whichever generic container `base_name` denotes (a generic class or a generic
    /// discriminated union), so nested generic types in field/argument positions are resolved.
    pub(super) fn ensure_type_instantiated(
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
    pub(super) fn enum_member_value(&self, enum_name: &str, member: &str) -> Option<i32> {
        self.enum_table
            .get(enum_name)
            .and_then(|m| m.get(member))
            .copied()
    }

    /// Pass 0: register every (non-generic) struct and its methods; stash generic templates.
    pub(super) fn register_structs(
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

    /// True when the (unadorned) type name resolves to a declared value (`struct`) type.
    #[allow(dead_code)]
    pub(super) fn is_value_type_name(&self, name: &str) -> bool {
        self.struct_table
            .get_struct(name)
            .map(|s| s.is_value)
            .unwrap_or(false)
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

    /// Pass: analyze and register every top-level variable. Each initializer is type-checked in
    /// declaration order against the globals declared so far (forward references to later globals
    /// are not allowed) plus all already-registered functions/types. The resolved type is recorded
    /// in the module-global symbol scope so function bodies can resolve the variable, and surfaced
    /// to codegen via [`super::GlobalSymbol`].
    pub(super) fn register_globals(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        // A synthetic, parameterless, non-async "module init" supplies the parent-function context
        // that expression analysis requires; with no `this` parameter it is treated as outside any
        // type, so initializers cannot reach private members.
        let empty_body: &'a [crate::syntax::nodes::StatementNode<'a>] = &[];
        let init_fn = FunctionNode::new(
            Vec::new(),
            synthetic_token(TokenKind::IdentifierToken, "__module_init"),
            None,
            None,
            Vec::new(),
            empty_body,
            false,
        );

        for global in node.globals.iter() {
            diagnostics.file_path = file_path_string(&global.file_path);
            self.check_reserved_name(&global.name, "variable", diagnostics);

            if global.is_public && global.is_static {
                diagnostics.report_error(
                    format!(
                        "Top-level variable '{}' cannot be both 'public' and 'static': they request opposite linkage ('public' exposes it to other modules, 'static' pins it to module-internal linkage)",
                        global.name.text
                    ),
                    Some(global.name.position),
                );
            }

            if self.globals.iter().any(|g| g.name == global.name.text) {
                diagnostics.report_error(
                    format!(
                        "Top-level variable '{}' is already defined",
                        global.name.text
                    ),
                    Some(global.name.position),
                );
                continue;
            }

            let gtable = self.global_symbol_table.clone();
            self.hir_global_init_begin();
            let init_type = self
                .analyze_expression(&global.initializer, &init_fn, &gtable, diagnostics)
                .unwrap_or(Type::Void);
            self.hir_global_init_finish(&global.name.text);

            let resolved = match &global.declared_type {
                Some(declared) => {
                    let dt = declared.get_type();
                    let it = init_type.get_type();
                    let numeric = crate::syntax::nodes::types::is_numeric_primitive(&dt)
                        && crate::syntax::nodes::types::is_numeric_primitive(&it);
                    if !numeric && it != "void" && !self.type_str_assignable(&dt, &it) {
                        diagnostics.report_error(
                            format!(
                                "Top-level variable '{}' is declared '{}' but initialized with '{}'",
                                global.name.text, dt, it
                            ),
                            Some(global.name.position),
                        );
                    }
                    declared.clone()
                }
                None => init_type,
            };

            {
                let mut table = self.global_symbol_table.borrow_mut();
                let _ = table.add_symbol(global.name.text.clone(), resolved.clone());
                if global.is_const {
                    table.mark_const(global.name.text.clone());
                }
            }

            self.globals.push(super::GlobalSymbol {
                name: global.name.text.clone(),
                type_str: resolved.get_type(),
                is_const: global.is_const,
                is_public: global.is_public,
                is_static: global.is_static,
                file_path: global.file_path.clone(),
            });
            // Register the HIR slot now (in declaration order) so a subsequent global's initializer
            // can resolve this one as a `Binding::Global`.
            self.hir_register_global(&global.name.text, &resolved.get_type(), global.is_const);
        }
    }

    /// Pass 1: register every (non-generic) function signature; stash generic templates.
    pub(super) fn register_functions(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        for function in node.functions.iter() {
            diagnostics.file_path = file_path_string(&function.file_path);
            self.check_reserved_name(&function.name, "function", diagnostics);
            if function.generic_parameters.is_some() {
                self.type_ctx.register(
                    DefKind::Function,
                    &function.name.text,
                    generic_param_names(&function.generic_parameters),
                );
                self.generic_functions
                    .insert(function.name.text.clone(), function);
                continue;
            }
            if function.is_public {
                self.check_public_visibility(function, diagnostics);
            }
            let info = FunctionTableInfo::from(function);
            if let Err(e) =
                self.function_table
                    .add_overload(&function.name.text, info, &mut self.type_ctx)
            {
                diagnostics.report_error(e.to_string(), Some(function.name.position));
            }
        }
        // Register a distinct `DefId` for every non-generic function under its *emitted* name (the
        // bare base when unique, the signature-mangled key when overloaded). Deferred to here so the
        // full overload set is known: overloaded declarations must not collide on a single base def.
        for function in node.functions.iter() {
            if function.generic_parameters.is_some() {
                continue;
            }
            let param_types: Vec<String> = function
                .parameters
                .iter()
                .map(|p| p.type_.get_type())
                .collect();
            let emitted = self.function_table.resolve_emitted_name(
                &function.name.text,
                &param_types,
                &mut self.type_ctx,
            );
            self.type_ctx.register(DefKind::Function, &emitted, vec![]);
        }
        // The entry point is exported under the fixed name `main`. It may be declared as `main()`
        // or `main(args: string[])`, but not overloaded or given any other signature.
        if self.function_table.is_overloaded("main") {
            diagnostics.report_error("'main' cannot be overloaded".to_string(), None);
        } else if let Ok(info) = self.function_table.get_function(&"main".to_string()) {
            let ok = info.parameters.is_empty()
                || (info.parameters.len() == 1 && info.parameters[0] == "string[]");
            if !ok {
                diagnostics.report_error(
                    "'main' must be declared as 'main()' or 'main(args: string[])'".to_string(),
                    None,
                );
            }
        }
    }

    /// Ensures a `public` function does not leak a private (non-`public`) class through its
    /// signature, which would make the class unusable by the callers the function is exposed to.
    pub(super) fn check_public_visibility(
        &self,
        function: &FunctionNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        let signature_types = function
            .return_type
            .iter()
            .chain(function.parameters.iter().map(|p| &p.type_));
        for type_to_check in signature_types {
            let base_type_str = strip_nullable(strip_array(&type_to_check.get_type())).to_string();
            if let Some(struct_info) = self.struct_table.get_struct(&base_type_str) {
                if !struct_info.is_public {
                    diagnostics.report_error(
                        format!(
                            "Public function '{}' exposes private class '{}'",
                            function.name.text, base_type_str
                        ),
                        Some(function.name.position),
                    );
                }
            }
        }
    }

    /// Pass 2: analyze the body of every concrete function.
    pub(super) fn analyze_function_bodies(
        &mut self,
        node: &'a ProgramNode<'a>,
        symbol_table_map: &mut HashMap<String, Rc<RefCell<SymbolTable>>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        for function in node.functions.iter() {
            if function.generic_parameters.is_some() {
                continue;
            }
            // Extern functions have no body; their signature is enough for call-site checks.
            if function.is_extern {
                continue;
            }
            diagnostics.file_path = file_path_string(&function.file_path);
            let table = self.analyze_function(function, diagnostics)?;
            // Key the symbol table by the emitted name so overloaded functions (which share a
            // base name but emit distinct mangled names) each get their own entry, matching the
            // name codegen uses.
            let param_types: Vec<String> = function
                .parameters
                .iter()
                .map(|p| p.type_.get_type())
                .collect();
            let key = self.function_table.resolve_emitted_name(
                &function.name.text,
                &param_types,
                &mut self.type_ctx,
            );
            symbol_table_map.insert(key, table);
        }
        Ok(())
    }

    /// Passes 3 & 4 (combined fixpoint): analyze the bodies of every monomorphized generic
    /// function instance and every (de-sugared) struct method.
    ///
    /// Analyzing one body can lazily instantiate *more* generics — a struct method that uses
    /// `List<JsonValue>` queues new struct methods, and a builder that calls `List<JsonValue>()`
    /// queues a new generic function instance. The two feed each other, so we loop until neither
    /// the generic-function set nor the struct-method list grows. Both instantiation paths are
    /// idempotent (guarded by the struct/function tables), so this terminates.
    pub(super) fn analyze_pending_instantiations(
        &mut self,
        symbol_table_map: &mut HashMap<String, Rc<RefCell<SymbolTable>>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let mut processed_generics: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut method_index = 0;
        loop {
            let mut progressed = false;

            // Monomorphized generic function instances (e.g. `List<JsonValue>`, `swap_int_string`).
            let pending: Vec<String> = self
                .instantiated_generics
                .keys()
                .filter(|k| !processed_generics.contains(*k))
                .cloned()
                .collect();
            for mangled_name in pending {
                processed_generics.insert(mangled_name.clone());
                let (bindings, template) = match self.instantiated_generics.get(&mangled_name) {
                    Some((b, t)) => (b.clone(), *t),
                    None => continue,
                };
                diagnostics.file_path = file_path_string(&template.file_path);
                let table = self.with_generic_bindings(bindings, |s| {
                    s.analyze_function(template, diagnostics)
                })?;
                symbol_table_map.insert(mangled_name, table);
                progressed = true;
            }

            // De-sugared struct methods, including those for newly instantiated generic structs.
            while method_index < self.struct_methods.len() {
                let (method, bindings) = self.struct_methods[method_index].clone();
                method_index += 1;
                diagnostics.file_path = file_path_string(&method.file_path);
                let table = self
                    .with_generic_bindings(bindings, |s| s.analyze_function(method, diagnostics))?;
                // Key by the emitted name so overloaded methods each get a distinct entry (the
                // parameter list includes the implicit `this`).
                let param_types: Vec<String> = method
                    .parameters
                    .iter()
                    .map(|p| p.type_.get_type())
                    .collect();
                let key = self.function_table.resolve_emitted_name(
                    &method.name.text,
                    &param_types,
                    &mut self.type_ctx,
                );
                symbol_table_map.insert(key, table);
                progressed = true;
            }

            if !progressed {
                break;
            }
        }
        Ok(())
    }
    pub(super) fn ensure_struct_instantiated(
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
        if args.len() != params.len() {
            diagnostics.report_error(
                format!(
                    "Generic class '{}' expects {} type argument(s), but {} were provided",
                    base_name,
                    params.len(),
                    args.len()
                ),
                Some(*position),
            );
        }
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
