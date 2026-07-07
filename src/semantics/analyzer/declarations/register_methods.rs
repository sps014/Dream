//! Method and extension registration: registering a struct's own methods and `extend`-block methods
//! (`register_methods_for`), the set of extendable targets, stashing generic extension templates,
//! and validating object-protocol overrides and property accessors. These are `impl Analyzer`
//! methods, kept in the `declarations` module alongside the other top-level registration passes.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::function_table::FunctionTableInfo;
use crate::syntax::nodes::struct_node::StructDeclarationNode;
use crate::syntax::nodes::types::PRIMITIVE_TYPE_NAMES;
use crate::syntax::nodes::{FunctionNode, ProgramNode, Type};
use crate::types::method_fn;

impl<'a> Analyzer<'a> {
    pub(in crate::semantics::analyzer) fn register_struct_methods(
        &mut self,
        struct_decl: &'a StructDeclarationNode<'a>,
        struct_type_str: &str,
        bindings: &GenericBindings,
        diagnostics: &mut DiagnosticBag,
    ) {
        self.register_methods_for(struct_type_str, &struct_decl.methods, bindings, diagnostics);
    }

    /// Registers a list of methods against `target_type_str` (a struct, a monomorphized generic
    /// struct, or a primitive/`object` extended via an `extend` block). Each method is renamed to
    /// `{target}_{method}`, given an implicit `this` parameter of the target type, queued for
    /// codegen, and recorded in the function table. Shared by struct declarations and `extend`
    /// blocks so they lower identically.
    pub(in crate::semantics::analyzer) fn register_methods_for(
        &mut self,
        target_type_str: &str,
        methods: &'a [FunctionNode<'a>],
        bindings: &GenericBindings,
        diagnostics: &mut DiagnosticBag,
    ) {
        // Collect the mangled name + full parameter list (with the implicit `this`) of each method so
        // overloaded methods can be registered under their signature-mangled *emitted* names in a
        // second pass, once the whole overload set for this target is known.
        let mut registered: Vec<(String, Vec<String>)> = Vec::new();
        for method in methods {
            // Validate object-protocol overrides once (on the non-monomorphized declaration).
            if bindings.is_empty() {
                self.validate_protocol_override(method, diagnostics);
                self.validate_accessor(method, diagnostics);
            }
            // Property accessors (`get`/`set`) are registered under a `$`-tagged internal name that a
            // user identifier can never spell, so `obj.prop`/`obj.prop = v` resolve to them without a
            // regular method (or the indexer `get`/`set` hooks) ever colliding.
            let member_name = accessor_member_name(method);
            let mangled_name = method_fn(target_type_str, &member_name);
            self.type_ctx.register(
                DefKind::Function,
                &mangled_name,
                generic_param_names(&method.generic_parameters),
            );

            if method.generic_parameters.is_some() {
                self.generic_functions.insert(mangled_name.clone(), method);
            }

            let mut new_method = method.clone();
            new_method.name = synthetic_token(TokenKind::IdentifierToken, &mangled_name);

            if !bindings.is_empty() {
                Self::substitute_generic_signature(&mut new_method, bindings);
            }

            // Static methods have no implicit receiver; instance methods get `this` at index 0.
            if !new_method.is_static {
                new_method
                    .parameters
                    .insert(0, Self::make_this_param(target_type_str));
            }

            let param_types: Vec<String> = new_method
                .parameters
                .iter()
                .map(|p| p.type_.get_type())
                .collect();
            let method_ref = self.arena.alloc(new_method);
            self.struct_methods.push((method_ref, bindings.clone()));

            let info = FunctionTableInfo::from(method_ref);
            if let Err(e) =
                self.function_table
                    .add_overload(&mangled_name, info, &mut self.type_ctx)
            {
                diagnostics.report_error(e.to_string(), Some(method.name.position));
            }
            if method.generic_parameters.is_none() {
                registered.push((mangled_name, param_types));
            }
        }
        // Register a distinct `DefId` for each overloaded method under its emitted (signature-mangled)
        // name, so overloads don't collide on the single base-mangled def (mirrors free functions).
        for (mangled_name, param_types) in registered {
            let emitted = self.function_table.resolve_emitted_name(
                &mangled_name,
                &param_types,
                &mut self.type_ctx,
            );
            if emitted != mangled_name {
                self.type_ctx.register(DefKind::Function, &emitted, vec![]);
            }
        }
    }

    /// Returns true if `name` is a type that an `extend` block may attach methods to: a
    /// primitive (the shared [`PRIMITIVE_TYPE_NAMES`] list), the dynamic `object`/`js` reference
    /// types, a registered struct, a generic struct template, or an enum.
    pub(in crate::semantics::analyzer) fn is_extendable_target(&self, name: &str) -> bool {
        PRIMITIVE_TYPE_NAMES.contains(&name)
            || matches!(name, "object" | "js")
            || self.struct_table.get_struct(name).is_some()
            || self.generic_structs.contains_key(name)
            || self.enum_table.contains_key(name)
    }

    /// Pass: register every `extend Type { ... }` block's methods. Extension methods are lowered
    /// exactly like struct methods (`{target}_{method}` + implicit `this`) but the target's
    /// runtime representation is untouched (it is NOT added to the struct table), so primitives
    /// keep their value/reference semantics.
    pub(in crate::semantics::analyzer) fn register_extensions(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        for ext in node.extends.iter() {
            diagnostics.file_path = file_path_string(&ext.file_path);
            let target = ext.target.text.clone();
            // `sealed` types reject user-authored `extend` blocks. Compiler-synthesized extends
            // (interface defaults, `@json` converters) are exempt, so a sealed type may still
            // implement interfaces with default methods or derive `@json`.
            if !ext.is_synthesized && self.sealed_types.contains(&target) {
                diagnostics.report_error(
                    format!("Cannot extend sealed type '{}'", target),
                    Some(ext.target.position),
                );
                continue;
            }
            if ext.generic_parameters.is_some() {
                // Generic extend blocks were stashed by `stash_generic_extensions` and are attached
                // per instantiation in `ensure_*_instantiated`; here we only validate the target is
                // a known generic union or struct.
                if !self.generic_unions.contains_key(&target)
                    && !self.generic_structs.contains_key(&target)
                {
                    diagnostics.report_error(
                        format!(
                            "Cannot extend unknown generic type '{}' (no generic union or class by that name)",
                            target
                        ),
                        Some(ext.target.position),
                    );
                }
                continue;
            }
            if !self.is_extendable_target(&target) {
                diagnostics.report_error(
                    format!("Cannot extend unknown type '{}'", target),
                    Some(ext.target.position),
                );
                continue;
            }
            self.register_methods_for(&target, &ext.methods, &GenericBindings::new(), diagnostics);
            // An `extend Type : Iface { ... }` block records that its target implements the
            // interface(s), so the target (including a primitive like `int`) participates in
            // interface dispatch and satisfies generic constraints (`T : Comparable<T>`). The
            // block's own methods supply the required signatures.
            if !ext.implements.is_empty() {
                self.validate_implements(
                    &target,
                    &ext.implements,
                    &ext.methods,
                    &GenericBindings::new(),
                    ext.target.position,
                    diagnostics,
                );
            }
        }
    }

    /// Pre-pass: stash every generic `extend Type<...> { ... }` block keyed by its target type
    /// name, so the methods are available to monomorphize at the first instantiation of that type
    /// (which can happen as early as `register_enums`). Validation of the target is deferred to
    /// `register_extensions`, once all type templates are registered.
    pub(in crate::semantics::analyzer) fn stash_generic_extensions(&mut self, node: &'a ProgramNode<'a>) {
        for ext in node.extends.iter() {
            if ext.generic_parameters.is_some() {
                // A generic type may have several `extend` blocks (e.g. a base `extend List<T>` plus a
                // constrained `extend List<T : Comparable<T>>`); keep them all and attach each whose
                // constraints the concrete instance satisfies.
                self.generic_extends
                    .entry(ext.target.text.clone())
                    .or_default()
                    .push(ext);
            }
        }
    }

    /// Validates an `@override` object-protocol method: `@override` may only mark `to_string`
    /// / `hash_code`, those must be exported with the exact protocol signature, and a method
    /// that shadows a protocol name must carry `@override`.
    pub(in crate::semantics::analyzer) fn validate_protocol_override(
        &self,
        method: &FunctionNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        let name = method.name.text.as_str();

        // Constructors/destructors: `del` takes no parameters and neither declares a return type.
        if name == crate::syntax::nodes::types::DESTRUCTOR_NAME && !method.parameters.is_empty() {
            diagnostics.report_error(
                "destructor 'del' must not declare parameters".to_string(),
                Some(method.name.position),
            );
        }
        if crate::syntax::nodes::types::is_special_member_name(name) && method.return_type.is_some()
        {
            diagnostics.report_error(
                format!("'{}' must not declare a return type", name),
                Some(method.name.position),
            );
        }

        let is_protocol =
            name == crate::intrinsics::TO_STRING || name == crate::intrinsics::HASH_CODE;

        let is_override = method.attributes.iter().any(|a| a.name.text == "override");

        if is_override && !is_protocol {
            diagnostics.report_error(
                format!("'@override' can only be applied to object-protocol methods (to_string, hash_code), not '{}'", name),
                Some(method.name.position),
            );
            return;
        }

        if is_protocol && !is_override {
            diagnostics.report_error(
                format!(
                    "method '{}' overrides an object-protocol method; mark it with '@override'",
                    name
                ),
                Some(method.name.position),
            );
            return;
        }

        if is_override && is_protocol {
            if !method.is_public {
                diagnostics.report_error(
                    format!(
                        "overridden object-protocol method '{}' must be declared 'public'",
                        name
                    ),
                    Some(method.name.position),
                );
            }
            if !method.parameters.is_empty() {
                diagnostics.report_error(
                    format!(
                        "overridden object-protocol method '{}' must not declare parameters",
                        name
                    ),
                    Some(method.name.position),
                );
            }
            let return_type = method.return_type.as_ref().map(|t| t.get_type());
            let expected = if name == "to_string" { "string" } else { "int" };
            if return_type.as_deref() != Some(expected) {
                diagnostics.report_error(
                    format!("overridden '{}' must return '{}'", name, expected),
                    Some(method.name.position),
                );
            }
        }
    }

    /// Validates a TypeScript-style property accessor (`get`/`set`): a getter takes no parameters
    /// and returns a non-`void` value; a setter takes exactly one parameter; neither may be `static`
    /// or `async`. Non-accessor methods are ignored.
    pub(in crate::semantics::analyzer) fn validate_accessor(
        &self,
        method: &FunctionNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        let Some(kind) = method.accessor else {
            return;
        };
        let prop = &method.name.text;
        // Static accessors are permitted: `static get`/`static set` are read/written through the
        // type (`Type.prop` / `Type.prop = v`) with no `this`. `async` accessors are not: a getter
        // read must yield the property value directly, not a `Future`.
        if method.is_async {
            diagnostics.report_error(
                format!("property accessor '{}' cannot be 'async'", prop),
                Some(method.name.position),
            );
        }
        match kind {
            crate::syntax::nodes::function::AccessorKind::Get => {
                if !method.parameters.is_empty() {
                    diagnostics.report_error(
                        format!("getter '{}' must not declare parameters", prop),
                        Some(method.name.position),
                    );
                }
                if matches!(method.return_type, None | Some(Type::Void)) {
                    diagnostics.report_error(
                        format!("getter '{}' must declare a non-void return type", prop),
                        Some(method.name.position),
                    );
                }
            }
            crate::syntax::nodes::function::AccessorKind::Set => {
                if method.parameters.len() != 1 {
                    diagnostics.report_error(
                        format!("setter '{}' must declare exactly one parameter", prop),
                        Some(method.name.position),
                    );
                }
            }
        }
    }
}
