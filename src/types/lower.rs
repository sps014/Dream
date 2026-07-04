//! [`TypeCtx`]: the analyzer-facing bundle of the [`TypeInterner`] and [`DefTable`], plus the
//! lowering from the AST [`Type`] to an interned [`TypeId`]. This is the bridge that lets the rest
//! of the compiler stop threading stringly-typed names around: declarations register their nominal
//! defs here, and every AST type annotation is lowered through [`TypeCtx::lower`].

use super::{DefKind, DefTable, PrimTy, TypeId, TypeInterner};
use dream_syntax::nodes::types::{mangle_generic, Type};
use indexmap::IndexMap;

/// Owns the interner and def table and remembers which nominal base names are structs/unions/enums
/// so AST `Type::Struct(name, args)` can be lowered to the right [`TyKind`](super::TyKind).
#[derive(Debug, Default)]
pub struct TypeCtx {
    pub interner: TypeInterner,
    pub defs: DefTable,
    /// Base name -> declared kind, used to disambiguate `Type::Struct` (which the parser also emits
    /// for unions and enums, since they are bare identifiers syntactically).
    nominal: IndexMap<String, DefKind>,
    /// Mangled monomorphization name (`List_JsonValue`) -> the canonical interned id of that generic
    /// instance (`Struct(List_def, [JsonValue])`). The analyzer registers each instantiation here so
    /// the pre-mangled bare spelling and the structured `List<JsonValue>` spelling lower to the same
    /// [`TypeId`] (the legacy pipeline uses both interchangeably).
    instances: IndexMap<String, TypeId>,
}

impl TypeCtx {
    pub fn new() -> Self {
        TypeCtx {
            interner: TypeInterner::new(),
            defs: DefTable::new(),
            nominal: IndexMap::new(),
            instances: IndexMap::new(),
        }
    }

    pub fn register(
        &mut self,
        kind: DefKind,
        name: &str,
        generic_params: Vec<String>,
    ) -> super::DefId {
        self.nominal.insert(name.to_string(), kind);
        self.defs.intern(kind, name, generic_params)
    }

    pub fn nominal_kind(&self, name: &str) -> Option<DefKind> {
        self.nominal.get(name).copied()
    }

    /// Records a generic instantiation so its mangled bare name canonicalizes to the structured
    /// `(base def, args)` id. `kind` is the base's kind (`Struct`/`Union`), `base` its source name,
    /// and `args` the concrete type arguments. Returns the canonical id. Idempotent.
    ///
    /// The mangled name is identity-defining, so the first registration wins: a later call whose
    /// `base` is itself the already-mangled name with no args (e.g. a field access on a value typed
    /// `Box_string`, which lowers `("Box_string", [])` rather than `("Box", [string])`) must not
    /// clobber the canonical `(base def, args)` id with a bogus nominal `struct_ty(Box_string, [])`.
    pub fn register_instance(&mut self, kind: DefKind, base: &str, args: &[Type]) -> TypeId {
        let mangled = mangle_generic(base, args);
        if let Some(&id) = self.instances.get(&mangled) {
            return id;
        }
        let arg_ids: Vec<TypeId> = args.iter().map(|a| self.lower(a)).collect();
        let def = self.defs.intern(kind, base, vec![]);
        let id = match kind {
            DefKind::Union => self.interner.union_ty(def, arg_ids),
            DefKind::Interface => self.interner.interface_ty(def, arg_ids),
            _ => self.interner.struct_ty(def, arg_ids),
        };
        self.instances.insert(mangled, id);
        id
    }

    /// Lowers an AST type to an interned id with no generic substitution in scope.
    pub fn lower(&mut self, ty: &Type) -> TypeId {
        self.lower_with(ty, &IndexMap::new())
    }

    /// Lowers a bare type-name string (possibly suffixed `T[]`/`T?`, a primitive spelling, or a
    /// pre-mangled generic instance) to an interned id. Bridges the legacy string-typed signatures
    /// and tables onto the structured type system.
    pub fn lower_str(&mut self, name: &str) -> TypeId {
        self.lower_name(name, &IndexMap::new())
    }

    /// Lowers a bare type-name string, substituting any in-scope generic parameter names (`T`) with
    /// their bound concrete id. The string-form counterpart of [`Self::lower_with`], used where a
    /// monomorphized body still carries a stringly-reconstructed type spelling that names a generic
    /// parameter (`T`, `T[]`, `T?`).
    pub fn lower_str_with(&mut self, name: &str, bindings: &IndexMap<String, TypeId>) -> TypeId {
        self.lower_name(name, bindings)
    }

    /// Lowers an AST type, substituting any in-scope generic parameter names (`T`) with the bound
    /// concrete id. Unbound generic names lower to the poison `Error` type.
    pub fn lower_with(&mut self, ty: &Type, bindings: &IndexMap<String, TypeId>) -> TypeId {
        match ty {
            Type::Integer(_) => self.interner.prim(PrimTy::Int),
            Type::UInt(_) => self.interner.prim(PrimTy::UInt),
            Type::Long(_) => self.interner.prim(PrimTy::Long),
            Type::ULong(_) => self.interner.prim(PrimTy::ULong),
            Type::Byte(_) => self.interner.prim(PrimTy::Byte),
            Type::Float(_) => self.interner.prim(PrimTy::Float),
            Type::Double(_) => self.interner.prim(PrimTy::Double),
            Type::Boolean(_) => self.interner.prim(PrimTy::Bool),
            Type::Char(_) => self.interner.prim(PrimTy::Char),
            Type::String(_) => self.interner.prim(PrimTy::String),
            Type::Object(_) => self.interner.object(),
            Type::Void => self.interner.void(),
            Type::Unknown => self.interner.error(),
            Type::Array(inner) => {
                let e = self.lower_with(inner, bindings);
                self.interner.array(e)
            }
            Type::Nullable(inner) => {
                let i = self.lower_with(inner, bindings);
                self.interner.nullable(i)
            }
            Type::Function(params, ret) => {
                let ps = params
                    .iter()
                    .map(|p| self.lower_with(p, bindings))
                    .collect();
                let r = self.lower_with(ret, bindings);
                self.interner.func(ps, r)
            }
            Type::Generic(name) => bindings
                .get(name)
                .copied()
                .unwrap_or_else(|| self.interner.error()),
            Type::Struct(token, generic_args) => {
                let name = &token.text;
                if let Some(bound) = bindings.get(name) {
                    return *bound;
                }
                // A bare name (no structured args) may be stringly-reconstructed and encode array/
                // nullable suffixes, a primitive spelling, or a pre-mangled generic instance; route
                // it through name-based lowering so every spelling of a type interns identically.
                if generic_args.is_none() {
                    return self.lower_name(name, bindings);
                }
                let args: Vec<TypeId> = generic_args
                    .as_ref()
                    .map(|gs| gs.iter().map(|g| self.lower_with(g, bindings)).collect())
                    .unwrap_or_default();
                match self.nominal_kind(name) {
                    Some(DefKind::Enum) => {
                        let def = self.defs.intern(DefKind::Enum, name, vec![]);
                        self.interner.enum_ty(def)
                    }
                    Some(DefKind::Union) => {
                        let def = self.defs.intern(DefKind::Union, name, vec![]);
                        self.interner.union_ty(def, args)
                    }
                    Some(DefKind::Interface) => {
                        let def = self.defs.intern(DefKind::Interface, name, vec![]);
                        self.interner.interface_ty(def, args)
                    }
                    // Default unknown nominal names to structs (forward references during
                    // declaration registration land here before the def is seen).
                    _ => {
                        let def = self.defs.intern(DefKind::Struct, name, vec![]);
                        self.interner.struct_ty(def, args)
                    }
                }
            }
        }
    }

    /// Lowers a bare type *name* (as opposed to a structured AST node) to an interned id, absorbing
    /// the stringly-typed spellings the legacy pipeline still produces: array (`T[]`) and nullable
    /// (`T?`) suffixes, primitive names, `object`/`void`, pre-mangled generic instances, and nominal
    /// references. This keeps every spelling of the same type interning to one [`TypeId`].
    fn lower_name(&mut self, name: &str, bindings: &IndexMap<String, TypeId>) -> TypeId {
        if let Some(&bound) = bindings.get(name) {
            return bound;
        }
        if let Some(base) = name.strip_suffix("[]") {
            let inner = self.lower_name(base, bindings);
            return self.interner.array(inner);
        }
        if let Some(base) = name.strip_suffix('?') {
            let inner = self.lower_name(base, bindings);
            return self.interner.nullable(inner);
        }
        if let Some(prim) = PrimTy::from_name(name) {
            return self.interner.prim(prim);
        }
        match name {
            "object" => return self.interner.object(),
            "void" => return self.interner.void(),
            // The dynamic JS-interop type is a distinguished non-reference i32 handle, not a nominal
            // struct, so recognize its bare name here before the nominal fallback.
            "js" => return self.interner.js(),
            _ => {}
        }
        if let Some(&id) = self.instances.get(name) {
            return id;
        }
        match self.nominal_kind(name) {
            Some(DefKind::Enum) => {
                let def = self.defs.intern(DefKind::Enum, name, vec![]);
                self.interner.enum_ty(def)
            }
            Some(DefKind::Union) => {
                let def = self.defs.intern(DefKind::Union, name, vec![]);
                self.interner.union_ty(def, vec![])
            }
            Some(DefKind::Interface) => {
                let def = self.defs.intern(DefKind::Interface, name, vec![]);
                self.interner.interface_ty(def, vec![])
            }
            Some(DefKind::Struct) => {
                let def = self.defs.intern(DefKind::Struct, name, vec![]);
                self.interner.struct_ty(def, vec![])
            }
            // An unregistered name (or a function name used in type position) is not a known type.
            // Interning it as a nominal struct here is exactly the fragility hazard from the review:
            // a typo or interning-drift would silently mint a bogus type and miscompile. Lower it to
            // the poison `Error` type instead, which `compat.rs` already suppresses so the real
            // diagnostic (raised where the name was resolved) is what surfaces.
            Some(DefKind::Function) | None => self.interner.error(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::line_text::LineText;
    use crate::text::text_span::TextSpan;
    use crate::types::{display_name, TyKind};
    use dream_syntax::token::syntax_token::SyntaxToken;
    use dream_syntax::token::token_kind::TokenKind;

    fn ident(text: &str) -> SyntaxToken {
        let lt = LineText::new(String::new());
        let span = TextSpan::new((0, 0), &lt);
        SyntaxToken::new(TokenKind::IdentifierToken, span, text.to_string())
    }

    #[test]
    fn lowers_primitive_and_array() {
        let mut ctx = TypeCtx::new();
        let arr = Type::Array(Box::new(Type::Integer(ident("int"))));
        let id = ctx.lower(&arr);
        assert!(matches!(ctx.interner.kind(id), TyKind::Array(_)));
        assert_eq!(display_name(&ctx.interner, &ctx.defs, id), "int[]");
    }

    #[test]
    fn lowers_registered_struct_with_args() {
        let mut ctx = TypeCtx::new();
        ctx.register(DefKind::Struct, "Box", vec!["T".to_string()]);
        let ty = Type::Struct(ident("Box"), Some(vec![Type::Integer(ident("int"))]));
        let id = ctx.lower(&ty);
        assert_eq!(display_name(&ctx.interner, &ctx.defs, id), "Box<int>");
    }

    #[test]
    fn generic_binding_substitutes() {
        let mut ctx = TypeCtx::new();
        let mut bindings = IndexMap::new();
        let int = ctx.interner.int();
        bindings.insert("T".to_string(), int);
        let ty = Type::Generic("T".to_string());
        assert_eq!(ctx.lower_with(&ty, &bindings), int);
    }
}
