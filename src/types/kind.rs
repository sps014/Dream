//! The structural vocabulary of the type system: [`TyKind`] (the shape of an interned type) and
//! [`PrimTy`] (the scalar built-ins). A `TyKind` references nested types by [`TypeId`] and named
//! definitions by [`DefId`], so it is a flat, hash-consable value with no owned recursion.

use super::{DefId, TypeId};

/// The built-in scalar types. `String` is included here for naming convenience even though it is a
/// heap reference at runtime; reference-ness is decided by [`TyKind::is_reference`], not by `PrimTy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimTy {
    Int,
    UInt,
    Long,
    ULong,
    Byte,
    Float,
    Double,
    Bool,
    Char,
    String,
}

impl PrimTy {
    /// The surface spelling, matching the legacy `Type::get_type()` strings exactly so the two
    /// representations stay interchangeable during the migration.
    pub fn name(self) -> &'static str {
        match self {
            PrimTy::Int => "int",
            PrimTy::UInt => "uint",
            PrimTy::Long => "long",
            PrimTy::ULong => "ulong",
            PrimTy::Byte => "byte",
            PrimTy::Float => "float",
            PrimTy::Double => "double",
            PrimTy::Bool => "bool",
            PrimTy::Char => "char",
            PrimTy::String => "string",
        }
    }

    /// Parses a primitive from its surface spelling.
    pub fn from_name(name: &str) -> Option<PrimTy> {
        Some(match name {
            "int" => PrimTy::Int,
            "uint" => PrimTy::UInt,
            "long" => PrimTy::Long,
            "ulong" => PrimTy::ULong,
            "byte" => PrimTy::Byte,
            "float" => PrimTy::Float,
            "double" => PrimTy::Double,
            "bool" => PrimTy::Bool,
            "char" => PrimTy::Char,
            "string" => PrimTy::String,
            _ => return None,
        })
    }

    /// True for the numeric primitives (everything except `bool`, `char`, and `string`).
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            PrimTy::Int
                | PrimTy::UInt
                | PrimTy::Long
                | PrimTy::ULong
                | PrimTy::Byte
                | PrimTy::Float
                | PrimTy::Double
        )
    }

    /// True for the unsigned integer primitives, which select unsigned WASM ops.
    pub fn is_unsigned_integer(self) -> bool {
        matches!(self, PrimTy::Byte | PrimTy::UInt | PrimTy::ULong)
    }

    /// Byte size and alignment of a scalar value of this primitive when stored inline (a struct
    /// field, array element, or local): `bool`/`char`/`byte` occupy a single byte;
    /// `double`/`long`/`ulong` are 8 bytes; everything else (`int`, `uint`, `float`, and `string`,
    /// which is a 4-byte heap pointer) is a 4-byte word. Single source of truth for this rule,
    /// shared by the string-keyed [`crate::types::naming::value_size_align`] (used by the
    /// legacy/analyzer struct tables) and the `TypeId`-keyed [`crate::hir::scalar_size`] (used by
    /// HIR/MIR layout) so the two representations can never disagree on a primitive's width.
    pub fn size_align(self) -> (u32, u32) {
        match self {
            PrimTy::Bool | PrimTy::Char | PrimTy::Byte => (1, 1),
            PrimTy::Double | PrimTy::Long | PrimTy::ULong => (8, 8),
            PrimTy::Int | PrimTy::UInt | PrimTy::Float | PrimTy::String => (4, 4),
        }
    }
}

/// The shape of an interned type. Produced and deduplicated by
/// [`TypeInterner`](super::TypeInterner); never constructed with owned recursion (nested types are
/// `TypeId`s), so it is cheap to clone, hash, and compare.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    /// A scalar built-in.
    Prim(PrimTy),
    /// The universal top type (`object`): an `i32` pointer to a tagged heap block.
    Object,
    /// The unit/return-nothing type.
    Void,
    /// The poison type produced on a semantic error; assignable to and from everything so one
    /// mistake does not cascade. Never lowered (codegen does not run after an error).
    Error,
    /// `T[]`, a heap-allocated reference.
    Array(TypeId),
    /// `T?`, a value of `T` or `null`.
    Nullable(TypeId),
    /// A user struct/class definition applied to zero or more type arguments (monomorphization is
    /// keyed by `(DefId, args)` rather than a mangled name).
    Struct(DefId, Vec<TypeId>),
    /// A discriminated-union definition applied to type arguments.
    Union(DefId, Vec<TypeId>),
    /// An interface definition applied to zero or more type arguments. Represented at runtime as an
    /// `i32` pointer to a tagged heap block (identical to `object`): an interface value is just the
    /// concrete object, and method calls dispatch dynamically through the object's runtime tag.
    Interface(DefId, Vec<TypeId>),
    /// A C-style enum definition (no type arguments; values are `int` at runtime).
    Enum(DefId),
    /// A first-class function value `fun(params...): ret`, an `i32` table index at runtime.
    Func(Vec<TypeId>, TypeId),
    /// The dynamic JavaScript-interop type `js`: an opaque `i32` handle into the host's live-value
    /// registry (see `runtime/dream.js`). Member/method/index access on a `js` value binds
    /// dynamically at runtime, so the compiler performs no member resolution. It is *not* a heap
    /// object, so it is never reference-counted (like an enum, it lowers to a bare `i32`).
    Js,
}

impl TyKind {
    /// True if a value of this type is a heap-allocated, reference-counted object (strings, arrays,
    /// objects, structs, and unions). Nullable wrappers defer to their inner type via the interner;
    /// this method only inspects the immediate kind, so callers strip `Nullable` first when needed.
    pub fn is_reference(&self) -> bool {
        matches!(
            self,
            TyKind::Prim(PrimTy::String)
                | TyKind::Object
                | TyKind::Array(_)
                | TyKind::Struct(_, _)
                | TyKind::Union(_, _)
                | TyKind::Interface(_, _)
        )
    }
}
