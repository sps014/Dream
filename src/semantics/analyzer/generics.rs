use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::syntax::nodes::function::ParameterNode;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::{FunctionNode, Type};
use crate::syntax::token::token_kind::TokenKind;
use crate::text::text_span::TextSpan;

/// The coarse shape of a type name, as classified by [`Analyzer::name_shape`].
enum NameShape<'s> {
    /// `T[]`: always a heap-allocated array reference.
    Array,
    /// `string`: a heap-allocated reference (spelled as a primitive name for historical reasons).
    String,
    /// A non-`string` scalar primitive (`int`, `bool`, `byte`, ...), stored inline.
    Primitive,
    /// A declared nominal `struct`/`class`; [`crate::semantics::struct_table::StructInfo::is_value`]
    /// says which.
    Nominal(&'s crate::semantics::struct_table::StructInfo),
    /// Not a recognized name (an interface, `object`/`js`, or an unresolved/generic name).
    Unknown,
}

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
            Type::Generic(name) if name == param_name => Some(arg.to_string()),
            Type::Array(inner) => {
                if let Some(arg_inner) = arg.strip_suffix("[]") {
                    Self::match_generic_type(inner, arg_inner, param_name)
                } else {
                    None
                }
            }
            // `Future<TOut>` / `List<T>` against `Future_int` or `Future<int>` spellings.
            Type::Struct(token, Some(args)) => {
                if let Some(inner_args) = Self::split_generic_type_str(arg, &token.text) {
                    if args.len() != inner_args.len() {
                        return None;
                    }
                    for (f, a) in args.iter().zip(inner_args.iter()) {
                        if let Some(c) = Self::match_generic_type(f, a, param_name) {
                            return Some(c);
                        }
                    }
                }
                None
            }
            // `fun(TIn): TOut` against `fun(int):int` — recurse into params and the return type.
            // An `async fun(...): T` value is typed as `fun(...): T` until a `Future<T>` context
            // wraps it, so also accept `fun(...): Future<TOut>` against a bare `T` return.
            Type::Function(formals, ret) => {
                let (arg_params, arg_ret) = Self::split_fun_type_str(arg)?;
                if formals.len() != arg_params.len() {
                    return None;
                }
                for (f, a) in formals.iter().zip(arg_params.iter()) {
                    if let Some(c) = Self::match_generic_type(f, a, param_name) {
                        return Some(c);
                    }
                }
                if let Some(c) = Self::match_generic_type(ret, arg_ret, param_name) {
                    return Some(c);
                }
                // `Future<TOut>` formal return vs bare `T` actual (async fun sugar).
                if let Type::Struct(token, Some(args)) = ret.as_ref() {
                    if token.text == "Future" && args.len() == 1 {
                        return Self::match_generic_type(&args[0], arg_ret, param_name);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Splits `Future_int` / `Future<int>` (when `base` is `Future`) into the type-argument
    /// spellings, or `None` if `s` is not an application of `base`.
    fn split_generic_type_str<'b>(s: &'b str, base: &str) -> Option<Vec<&'b str>> {
        if let Some(rest) = s.strip_prefix(base).and_then(|r| r.strip_prefix('<')) {
            let inner = rest.strip_suffix('>')?;
            return Some(Self::split_top_level_args(inner));
        }
        if let Some(rest) = s.strip_prefix(base).and_then(|r| r.strip_prefix('_')) {
            // Mangling joins args with `_`; nested generics are rare here and already flattened.
            return Some(rest.split('_').filter(|p| !p.is_empty()).collect());
        }
        None
    }

    fn split_top_level_args(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        let mut start = 0;
        for (i, ch) in s.char_indices() {
            match ch {
                '(' | '<' => depth += 1,
                ')' | '>' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(s[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
        }
        out.push(s[start..].trim());
        out
    }

    /// Splits a `fun(a,b):ret` spelling into `([a, b], ret)`, or `None` if `s` is not a fun type.
    fn split_fun_type_str(s: &str) -> Option<(Vec<&str>, &str)> {
        let rest = s.strip_prefix("fun(")?;
        let close = rest.find(')')?;
        let params_str = &rest[..close];
        let after = rest[close + 1..].strip_prefix(':')?;
        let params = if params_str.is_empty() {
            Vec::new()
        } else {
            Self::split_top_level_args(params_str)
        };
        Some((params, after.trim()))
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
                Self::check_generic_arity(
                    "function",
                    &template.name.text,
                    gen_params.len(),
                    generics.len(),
                    position,
                    diagnostics,
                );
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
    /// monomorphization bindings, recursing through array wrappers (`T`, `T[]`).
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
                            ("struct", "it is not a value type")
                        }
                        crate::syntax::nodes::ConstraintKind::Unmanaged => (
                            "unmanaged",
                            "it is not a blittable value type (it contains reference-typed fields, or is a reference type)",
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
    /// `struct` requires a *value type* (a non-`string` scalar primitive or a value `struct`),
    /// which may still hold reference-typed fields; `unmanaged` additionally requires it to be
    /// *blittable* (recursively only value fields, no inner heap pointers - a self-contained run of
    /// bytes); `class` requires a reference type.
    pub(super) fn type_satisfies_kind(
        &self,
        concrete: &Type,
        kind: crate::syntax::nodes::ConstraintKind,
    ) -> bool {
        let name = concrete.get_type();
        match kind {
            crate::syntax::nodes::ConstraintKind::Struct => self.name_is_value_type(&name),
            crate::syntax::nodes::ConstraintKind::Unmanaged => {
                self.name_is_blittable_value(&name, &mut std::collections::HashSet::new())
            }
            crate::syntax::nodes::ConstraintKind::Class => self.name_is_reference_type(&name),
        }
    }

    /// True when `ty` is a still-unbound generic type parameter (an unresolved name, e.g. `TIn`
    /// inside a generic template's own body before monomorphization substitutes it) rather than a
    /// concrete, checkable type. Lets a call-site intrinsic (e.g. `Bytes.toWire<T>`) defer its own
    /// validation to the monomorphized pass instead of misreporting the bare parameter name as
    /// failing a constraint it hasn't been bound long enough to actually satisfy or violate.
    pub(super) fn is_unresolved_generic_type(&self, ty: &Type) -> bool {
        matches!(self.name_shape(&ty.get_type()), NameShape::Unknown)
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
                    "'{}' requires an unmanaged (blittable) type, but '{}' is not (it is a reference type, or contains reference-typed fields)",
                    who,
                    ty.get_type()
                ),
                Some(*position),
            );
        }
    }

    /// Like [`Self::require_unmanaged`], but also accepts `T[]` when its element type is itself
    /// unmanaged: the wire/byte-blit intrinsics (`Bytes.of`/`to`, `Bytes.toWire`/`fromWire`) copy
    /// such an array's raw element bytes (a dynamic-length `memory.copy`, never the array pointer
    /// itself), which is exactly as safe as blitting a single blittable value — no reference,
    /// aliasing, or refcounting is involved. Only one level of array is allowed: an array of arrays
    /// is never blittable, since its element type is itself a reference.
    pub(super) fn require_unmanaged_or_array(
        &self,
        ty: &Type,
        who: &str,
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        if let Type::Array(inner) = ty {
            if self.type_satisfies_kind(inner, crate::syntax::nodes::ConstraintKind::Unmanaged) {
                return;
            }
        }
        self.require_unmanaged(ty, who, position, diagnostics);
    }

    /// The coarse shape of a type name, as needed to decide the `struct`/
    /// `unmanaged`/`class` kind constraints. Single source of truth for "what counts as an array /
    /// a string / a scalar primitive / a declared value struct", shared by
    /// [`Self::name_is_value_type`], [`Self::name_is_blittable_value`], and
    /// [`Self::name_is_reference_type`] below (previously each re-derived this classification from
    /// the raw name independently).
    fn name_shape<'s>(&'s self, name: &str) -> NameShape<'s> {
        if name.ends_with("[]") {
            return NameShape::Array;
        }
        if name == "string" {
            return NameShape::String;
        }
        if crate::syntax::nodes::types::is_boxable_primitive(name) {
            return NameShape::Primitive;
        }
        match self.struct_table.get_struct(name) {
            Some(info) => NameShape::Nominal(info),
            // An interface, `object`/`js`, or an unresolved/generic name: not a known value shape,
            // so it falls back to a reference type below (the pre-existing default).
            None => NameShape::Unknown,
        }
    }

    /// True when `name` is a value type: a non-`string` scalar primitive or a declared value
    /// `struct` (regardless of whether its fields are references). The complement of
    /// [`Self::name_is_reference_type`] for known nominal types.
    fn name_is_value_type(&self, name: &str) -> bool {
        match self.name_shape(name) {
            NameShape::Primitive => true,
            NameShape::Nominal(info) => info.is_value,
            NameShape::Array | NameShape::String | NameShape::Unknown => false,
        }
    }

    fn name_is_blittable_value(
        &self,
        name: &str,
        seen: &mut std::collections::HashSet<String>,
    ) -> bool {
        match self.name_shape(name) {
            NameShape::Array | NameShape::String | NameShape::Unknown => false,
            NameShape::Primitive => true,
            NameShape::Nominal(info) => {
                if !info.is_value {
                    return false; // class / reference struct
                }
                if !seen.insert(name.to_string()) {
                    return true; // cycle guard (value structs cannot actually recurse by value)
                }
                for f in info.fields.values() {
                    let fname = f.type_.get_type();
                    if !self.name_is_blittable_value(&fname, seen) {
                        return false;
                    }
                }
                true
            }
        }
    }

    fn name_is_reference_type(&self, name: &str) -> bool {
        match self.name_shape(name) {
            NameShape::Array | NameShape::String | NameShape::Unknown => true,
            NameShape::Primitive => false,
            NameShape::Nominal(info) => !info.is_value,
        }
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
            None => concrete.get_type(),
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
