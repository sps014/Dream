use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::syntax::nodes::function::ParameterNode;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::{FunctionNode, Type};
use crate::syntax::token::token_kind::TokenKind;
use crate::text::text_span::TextSpan;

impl<'a> Analyzer<'a> {
    /// Substitutes every generic parameter appearing in a method's parameter or return types
    /// with its concrete type, according to the monomorphization bindings.
    pub(super) fn substitute_generic_signature(
        method: &mut FunctionNode<'a>,
        bindings: &GenericBindings,
    ) {
        for param in &mut method.parameters {
            param.type_ = Self::monomorphize_type(&param.type_, bindings);
        }
        if let Some(ret) = &method.return_type {
            method.return_type = Some(Self::monomorphize_type(ret, bindings));
        }
    }

    fn match_generic_type(formal: &Type, arg: &str, param_name: &str) -> Option<String> {
        match formal {
            Type::Struct(token, None) if token.text == param_name => Some(arg.to_string()),
            Type::Array(inner) => {
                if let Some(arg_inner) = arg.strip_suffix("[]") {
                    Self::match_generic_type(inner, arg_inner, param_name)
                } else {
                    None
                }
            }
            Type::Nullable(inner) => {
                if let Some(arg_inner) = arg.strip_suffix('?') {
                    Self::match_generic_type(inner, arg_inner, param_name)
                } else {
                    Self::match_generic_type(inner, arg, param_name)
                }
            }
            _ => None,
        }
    }

    /// Determines the concrete type bound to each generic parameter of `template` for one call.
    /// Uses explicit type arguments when given (arity-checked); otherwise infers each parameter
    /// from the actual argument passed to the first formal parameter that is exactly that
    /// parameter. Parameters that cannot be inferred produce a diagnostic.
    pub(super) fn infer_generic_bindings(
        &self,
        template: &FunctionNode<'a>,
        generic_args: &Option<Vec<Type>>,
        params_types: &[String],
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) -> GenericBindings {
        let gen_params = template.generic_parameters.as_deref().unwrap_or(&[]);

        if let Some(generics) = generic_args {
            if !generics.is_empty() {
                if generics.len() != gen_params.len() {
                    diagnostics.report_error(
                        format!("Generic function '{}' expects {} type argument(s), but {} were provided", template.name.text, gen_params.len(), generics.len()),
                        Some(*position),
                    );
                }
                return gen_params
                    .iter()
                    .zip(generics.iter())
                    .map(|(param, arg)| (param.text.clone(), arg.clone()))
                    .collect();
            }
        }

        gen_params.iter().map(|param| {
            let concrete = template.parameters.iter().enumerate().find_map(|(i, formal)| {
                params_types.get(i).and_then(|arg| {
                    Self::match_generic_type(&formal.type_, arg, &param.text)
                })
            });
            match concrete {
                Some(concrete) => (param.text.clone(), Self::concrete_type_from_str(&concrete)),
                None => {
                    diagnostics.report_error(
                        format!("Cannot infer generic parameter '{}' of function '{}'; specify type arguments explicitly", param.text, template.name.text),
                        Some(*position),
                    );
                    (param.text.clone(), Type::Void)
                }
            }
        }).collect()
    }

    /// Returns `ty` with any generic parameter substituted for its concrete type per the
    /// monomorphization bindings, recursing through array and nullable wrappers (`T`, `T[]`, `T?`).
    pub(super) fn monomorphize_type(ty: &Type, bindings: &GenericBindings) -> Type {
        match ty {
            Type::Struct(token, None) => match lookup_binding(bindings, &token.text) {
                Some(concrete) => concrete,
                None => ty.clone(),
            },
            // A generic struct applied to type arguments (e.g. `List<T>`): substitute inside the
            // arguments so a generic function/method returning `List<T>` resolves to `List<int>`.
            Type::Struct(token, Some(args)) => Type::Struct(
                token.clone(),
                Some(
                    args.iter()
                        .map(|a| Self::monomorphize_type(a, bindings))
                        .collect(),
                ),
            ),
            Type::Array(inner) => Type::Array(Box::new(Self::monomorphize_type(inner, bindings))),
            Type::Nullable(inner) => {
                Type::Nullable(Box::new(Self::monomorphize_type(inner, bindings)))
            }
            // First-class function types (`fun(T, T): int`) must substitute inside their parameter
            // and return types so a monomorphized callback param (e.g. `sort_by`'s comparator)
            // type-checks against concrete arguments.
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|p| Self::monomorphize_type(p, bindings))
                    .collect(),
                Box::new(Self::monomorphize_type(ret, bindings)),
            ),
            _ => ty.clone(),
        }
    }

    /// Verifies that each concrete type bound by `bindings` satisfies its declared generic
    /// `constraints` (`T : Comparable<T>` etc.), reporting a clear error otherwise. Each bound is
    /// substituted with the same bindings so `Comparable<T>` becomes `Comparable<int>` before the
    /// `implements` lookup; the concrete argument must implement that (mangled) interface.
    pub(super) fn verify_generic_constraints(
        &self,
        constraints: &[crate::syntax::nodes::GenericConstraint],
        bindings: &GenericBindings,
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        for constraint in constraints {
            let Some(concrete) = bindings.get(&constraint.param.text) else {
                continue;
            };
            for bound in &constraint.bounds {
                if !self.type_satisfies_bound(concrete, bound, bindings) {
                    diagnostics.report_error(
                        format!(
                            "type '{}' does not satisfy the constraint '{}' on generic parameter '{}' (it does not implement that interface)",
                            concrete.get_type(),
                            bound.get_type(),
                            constraint.param.text
                        ),
                        Some(*position),
                    );
                }
            }
            for kind in &constraint.kinds {
                if !self.type_satisfies_kind(concrete, *kind) {
                    let (want, why) = match kind {
                        crate::syntax::nodes::ConstraintKind::Struct => {
                            ("struct", "it is not a (non-nullable) value type")
                        }
                        crate::syntax::nodes::ConstraintKind::Unmanaged => (
                            "unmanaged",
                            "it is not a blittable value type (it contains reference-typed fields, or is nullable/a reference type)",
                        ),
                        crate::syntax::nodes::ConstraintKind::Class => {
                            ("class", "it is not a reference type")
                        }
                    };
                    diagnostics.report_error(
                        format!(
                            "type '{}' does not satisfy the '{}' constraint on generic parameter '{}' ({})",
                            concrete.get_type(),
                            want,
                            constraint.param.text,
                            why
                        ),
                        Some(*position),
                    );
                }
            }
        }
    }

    /// True when `concrete` satisfies a `struct`/`unmanaged`/`class` kind constraint (C#-aligned):
    /// `struct` requires a non-nullable *value type* (a non-`string` scalar primitive or a value
    /// `struct`), which may still hold reference-typed fields; `unmanaged` additionally requires it
    /// to be *blittable* (recursively only value fields, no inner heap pointers - a self-contained
    /// run of bytes); `class` requires a reference type.
    pub(super) fn type_satisfies_kind(
        &self,
        concrete: &Type,
        kind: crate::syntax::nodes::ConstraintKind,
    ) -> bool {
        let name = concrete.get_type();
        let base = crate::syntax::nodes::types::strip_nullable(&name);
        match kind {
            // A nullable value type is boxed to a heap pointer, so it is not a plain value type.
            crate::syntax::nodes::ConstraintKind::Struct => {
                !name.ends_with('?') && self.name_is_value_type(base)
            }
            crate::syntax::nodes::ConstraintKind::Unmanaged => {
                !name.ends_with('?')
                    && self.name_is_blittable_value(base, &mut std::collections::HashSet::new())
            }
            crate::syntax::nodes::ConstraintKind::Class => self.name_is_reference_type(base),
        }
    }

    /// Reports an error unless `ty` satisfies the `unmanaged` (blittable) kind. Used by the raw
    /// byte-blit intrinsics (`Bytes.of`/`Bytes.to`), whose generic bound is verified here rather
    /// than through the normal call-site constraint path (which they bypass).
    pub(super) fn require_unmanaged(
        &self,
        ty: &Type,
        who: &str,
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        if !self.type_satisfies_kind(ty, crate::syntax::nodes::ConstraintKind::Unmanaged) {
            diagnostics.report_error(
                format!(
                    "'{}' requires an unmanaged (blittable) type, but '{}' is not (it is a reference type, is nullable, or contains reference-typed fields)",
                    who,
                    ty.get_type()
                ),
                Some(*position),
            );
        }
    }

    /// True when `name` is a value type: a non-`string` scalar primitive or a declared value
    /// `struct` (regardless of whether its fields are references). The complement of
    /// [`Self::name_is_reference_type`] for known nominal types.
    fn name_is_value_type(&self, name: &str) -> bool {
        if name.ends_with("[]") || name == "string" {
            return false; // arrays and strings are heap references
        }
        if crate::syntax::nodes::types::is_boxable_primitive(name) {
            return true; // non-string scalar primitive
        }
        self.struct_table
            .get_struct(name)
            .map(|s| s.is_value)
            .unwrap_or(false) // class / unknown / generic param
    }

    fn name_is_blittable_value(
        &self,
        name: &str,
        seen: &mut std::collections::HashSet<String>,
    ) -> bool {
        if name.ends_with("[]") {
            return false; // arrays are heap references
        }
        if crate::syntax::nodes::types::is_boxable_primitive(name) {
            return true; // non-string scalar primitive
        }
        if name == "string" {
            return false;
        }
        let Some(info) = self.struct_table.get_struct(name) else {
            return false; // class / unknown / generic param
        };
        if !info.is_value {
            return false;
        }
        if !seen.insert(name.to_string()) {
            return true; // cycle guard (value structs cannot actually recurse by value)
        }
        for f in info.fields.values() {
            let fname = f.type_.get_type();
            if fname.ends_with('?') {
                return false; // nullable field is a boxed/nullable pointer
            }
            let fbase = crate::syntax::nodes::types::strip_nullable(&fname);
            if !self.name_is_blittable_value(fbase, seen) {
                return false;
            }
        }
        true
    }

    fn name_is_reference_type(&self, name: &str) -> bool {
        if name.ends_with("[]") || name == "string" {
            return true;
        }
        if crate::syntax::nodes::types::is_boxable_primitive(name) {
            return false;
        }
        // A declared value `struct` is not a reference; a `class` is. Unions/object/js/interfaces
        // and unknown names default to reference types.
        self.struct_table
            .get_struct(name)
            .map(|s| !s.is_value)
            .unwrap_or(true)
    }

    /// True when `concrete` implements the interface named by `bound` (after substituting the
    /// monomorphization `bindings` into `bound`, e.g. `Comparable<T>` -> `Comparable<int>`).
    pub(super) fn type_satisfies_bound(
        &self,
        concrete: &Type,
        bound: &Type,
        bindings: &GenericBindings,
    ) -> bool {
        let bound = substitute_generic_type(bound, bindings);
        let iface = match Self::resolve_struct_parts(&bound) {
            Some((base, args)) if args.is_empty() => base,
            Some((base, args)) => mangle_generic(&base, &args),
            None => return false,
        };
        let concrete_name = match Self::resolve_struct_parts(concrete) {
            Some((base, args)) => mangle_generic(&base, &args),
            None => crate::syntax::nodes::types::strip_nullable(&concrete.get_type()).to_string(),
        };
        self.class_implements(&concrete_name, &iface)
    }

    /// Builds the implicit `this` parameter injected as the first argument of every method.
    /// For an extension method on a primitive, `this` is the primitive's value type (e.g.
    /// `int` -> `Type::Integer`, a stack value); for a struct it is the struct reference type.
    pub(super) fn make_this_param(struct_type_str: &str) -> ParameterNode {
        let token = synthetic_token(TokenKind::IdentifierToken, struct_type_str);
        let this_type = Type::from_token(token.clone()).unwrap_or(Type::Struct(token, None));
        ParameterNode::new(
            synthetic_token(TokenKind::IdentifierToken, "this"),
            this_type,
        )
    }
}
