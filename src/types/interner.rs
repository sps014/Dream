//! The [`TypeInterner`]: hash-conses [`TyKind`]s to compact [`TypeId`]s so type equality is a
//! single integer comparison and nested types are shared. The common nullary types are pre-interned
//! at construction and exposed as accessors.

use super::{DefId, PrimTy, TyKind, TypeId};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

/// Interns [`TyKind`]s to [`TypeId`]s. Structural equality of types reduces to `TypeId` equality
/// because identical kinds always intern to the same id.
#[derive(Debug)]
pub struct TypeInterner {
    kinds: Vec<TyKind>,
    dedup: IndexMap<TyKind, TypeId>,
    /// `DefId`s of `struct` (value) types. Consulted by [`Self::is_reference`] so a `Struct(def, _)`
    /// whose def is a value type is classified as a non-reference (stored inline, copy semantics).
    /// The interner has no access to the `DefTable`, so the value-ness is mirrored here.
    value_defs: HashSet<DefId>,
    /// Inline `(size, align)` in bytes of each value (`struct`) type, keyed by its (nullable-stripped)
    /// interned id. Populated once layouts are computed; consulted by `scalar_size` so a value struct
    /// stored as a field/element/local occupies its full inline footprint rather than a 4-byte pointer.
    value_layouts: HashMap<TypeId, (u32, u32)>,
    /// Interned ids of value *unions* (a data `enum` instance every one of whose variant payloads is
    /// value/primitive, e.g. `Option<int>`). Unlike value structs (marked per-`DefId`) value-ness is
    /// per-`TypeId`, because `Option<int>` (value) and `Option<string>` (heap) share one `DefId`.
    value_unions: HashSet<TypeId>,
}

impl Default for TypeInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeInterner {
    pub fn new() -> Self {
        let mut interner = TypeInterner {
            kinds: Vec::new(),
            dedup: IndexMap::new(),
            value_defs: HashSet::new(),
            value_layouts: HashMap::new(),
            value_unions: HashSet::new(),
        };
        // Pre-intern the nullary types so their ids are stable and cheap to reach.
        for prim in [
            PrimTy::Int,
            PrimTy::UInt,
            PrimTy::Long,
            PrimTy::ULong,
            PrimTy::Byte,
            PrimTy::Float,
            PrimTy::Double,
            PrimTy::Bool,
            PrimTy::Char,
            PrimTy::String,
        ] {
            interner.intern(TyKind::Prim(prim));
        }
        interner.intern(TyKind::Object);
        interner.intern(TyKind::Void);
        interner.intern(TyKind::Error);
        interner.intern(TyKind::Js);
        interner
    }

    pub fn intern(&mut self, kind: TyKind) -> TypeId {
        if let Some(&id) = self.dedup.get(&kind) {
            return id;
        }
        let id = TypeId(self.kinds.len() as u32);
        self.kinds.push(kind.clone());
        self.dedup.insert(kind, id);
        id
    }

    pub fn kind(&self, id: TypeId) -> &TyKind {
        &self.kinds[id.0 as usize]
    }

    pub fn prim(&mut self, prim: PrimTy) -> TypeId {
        self.intern(TyKind::Prim(prim))
    }

    pub fn array(&mut self, element: TypeId) -> TypeId {
        self.intern(TyKind::Array(element))
    }

    pub fn nullable(&mut self, inner: TypeId) -> TypeId {
        // `T??` collapses to `T?`; a nullable error/void is still itself.
        if let TyKind::Nullable(_) = self.kind(inner) {
            return inner;
        }
        self.intern(TyKind::Nullable(inner))
    }

    pub fn struct_ty(&mut self, def: DefId, args: Vec<TypeId>) -> TypeId {
        self.intern(TyKind::Struct(def, args))
    }

    pub fn union_ty(&mut self, def: DefId, args: Vec<TypeId>) -> TypeId {
        self.intern(TyKind::Union(def, args))
    }

    pub fn interface_ty(&mut self, def: DefId, args: Vec<TypeId>) -> TypeId {
        self.intern(TyKind::Interface(def, args))
    }

    pub fn enum_ty(&mut self, def: DefId) -> TypeId {
        self.intern(TyKind::Enum(def))
    }

    pub fn func(&mut self, params: Vec<TypeId>, ret: TypeId) -> TypeId {
        self.intern(TyKind::Func(params, ret))
    }

    // Accessors for the pre-interned nullary types. These rely on the construction order above.
    pub fn int(&self) -> TypeId {
        self.find(&TyKind::Prim(PrimTy::Int))
    }
    pub fn bool(&self) -> TypeId {
        self.find(&TyKind::Prim(PrimTy::Bool))
    }
    pub fn char(&self) -> TypeId {
        self.find(&TyKind::Prim(PrimTy::Char))
    }
    pub fn long(&self) -> TypeId {
        self.find(&TyKind::Prim(PrimTy::Long))
    }
    pub fn float(&self) -> TypeId {
        self.find(&TyKind::Prim(PrimTy::Float))
    }
    pub fn double(&self) -> TypeId {
        self.find(&TyKind::Prim(PrimTy::Double))
    }
    pub fn string(&self) -> TypeId {
        self.find(&TyKind::Prim(PrimTy::String))
    }
    pub fn object(&self) -> TypeId {
        self.find(&TyKind::Object)
    }
    pub fn void(&self) -> TypeId {
        self.find(&TyKind::Void)
    }
    pub fn error(&self) -> TypeId {
        self.find(&TyKind::Error)
    }
    /// The dynamic JS-interop type `js` (a non-reference `i32` handle).
    pub fn js(&self) -> TypeId {
        self.find(&TyKind::Js)
    }

    fn find(&self, kind: &TyKind) -> TypeId {
        self.dedup[kind]
    }

    /// The element type of an array, the inner type of a nullable, or `None` otherwise.
    pub fn unwrap_array(&self, id: TypeId) -> Option<TypeId> {
        match self.kind(id) {
            TyKind::Array(e) => Some(*e),
            _ => None,
        }
    }

    pub fn unwrap_nullable(&self, id: TypeId) -> Option<TypeId> {
        match self.kind(id) {
            TyKind::Nullable(inner) => Some(*inner),
            _ => None,
        }
    }

    /// Strips a single `Nullable` wrapper, returning the inner id (or the id unchanged).
    pub fn strip_nullable(&self, id: TypeId) -> TypeId {
        self.unwrap_nullable(id).unwrap_or(id)
    }

    /// Records `def` as a value (`struct`) type so [`Self::is_reference`] treats its instances as
    /// inline values rather than heap references. Idempotent.
    pub fn mark_value_def(&mut self, def: DefId) {
        self.value_defs.insert(def);
    }

    /// True when `def` names a value (`struct`) type.
    pub fn is_value_def(&self, def: DefId) -> bool {
        self.value_defs.contains(&def)
    }

    /// Records `id` (after stripping any nullable wrapper) as a value *union* type. Idempotent.
    pub fn mark_value_union(&mut self, id: TypeId) {
        let id = self.strip_nullable(id);
        self.value_unions.insert(id);
    }

    /// True when `id` names a value union (after stripping any nullable wrapper).
    pub fn is_value_union(&self, id: TypeId) -> bool {
        self.value_unions.contains(&self.strip_nullable(id))
    }

    /// True if `id` names a value type — a value (`struct`) type or a value union — after stripping
    /// any nullable wrapper. Both are stored inline with copy semantics rather than as heap
    /// references.
    ///
    /// A *nullable* value struct (`T?`) is the exception: it is stored as a nullable heap pointer to
    /// a boxed copy of `T` (so `null` has a representation), and therefore behaves as a reference,
    /// not an inline value. `is_nullable_boxed_value` distinguishes it.
    pub fn is_value_type(&self, id: TypeId) -> bool {
        if self.is_nullable_boxed_value(id) {
            return false;
        }
        let stripped = self.strip_nullable(id);
        if self.value_unions.contains(&stripped) {
            return true;
        }
        matches!(self.kind(stripped), TyKind::Struct(def, _) if self.value_defs.contains(def))
    }

    /// True when `id` is `T?` where `T` is a value (`struct`) type: represented at runtime as a
    /// nullable heap pointer to a boxed copy of `T`, so `null` is expressible. A value *union* is
    /// excluded (it carries its own inline discriminant and stays inline even when nullable).
    pub fn is_nullable_boxed_value(&self, id: TypeId) -> bool {
        let TyKind::Nullable(inner) = self.kind(id) else {
            return false;
        };
        let inner = self.strip_nullable(*inner);
        if self.value_unions.contains(&inner) {
            return false;
        }
        matches!(self.kind(inner), TyKind::Struct(def, _) if self.value_defs.contains(def))
    }

    /// Records the inline `(size, align)` of a value (`struct`) type. Keyed by the nullable-stripped
    /// id so a `T?` value struct resolves to the same footprint as `T`. Idempotent.
    pub fn set_value_layout(&mut self, id: TypeId, size: u32, align: u32) {
        let id = self.strip_nullable(id);
        self.value_layouts.insert(id, (size, align));
    }

    /// The recorded inline `(size, align)` of a value (`struct`) type, or `None` for reference types
    /// and value structs whose layout has not been computed yet.
    pub fn value_layout(&self, id: TypeId) -> Option<(u32, u32)> {
        self.value_layouts.get(&self.strip_nullable(id)).copied()
    }

    /// True if a value of `id` is a heap reference (after stripping any nullable wrapper). A
    /// `struct` (value) type is *not* a reference even though it is a `TyKind::Struct`.
    pub fn is_reference(&self, id: TypeId) -> bool {
        // A nullable value struct (`T?`) is boxed onto the heap, so it *is* a reference even though
        // the bare `T` is an inline value.
        if self.is_nullable_boxed_value(id) {
            return true;
        }
        let stripped = self.strip_nullable(id);
        // A value union is stored inline (not a heap reference) even though it is a `TyKind::Union`.
        if self.value_unions.contains(&stripped) {
            return false;
        }
        if let TyKind::Struct(def, _) = self.kind(stripped) {
            if self.value_defs.contains(def) {
                return false;
            }
        }
        self.kind(stripped).is_reference()
    }

    /// Iterates every interned type as `(id, kind)` in interning order (deterministic). Used by the
    /// backend to enumerate, e.g., all function types that need a `call_indirect` signature.
    pub fn iter_kinds(&self) -> impl Iterator<Item = (TypeId, &TyKind)> {
        self.kinds
            .iter()
            .enumerate()
            .map(|(i, k)| (TypeId(i as u32), k))
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}
