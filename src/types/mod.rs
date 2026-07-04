//! The structured type system: an interner that hash-conses type shapes to compact ids, a def table
//! that names nominal declarations, and structural relations (widening/assignability) over ids.
//!
//! This replaces the historical stringly-typed representation (`Type::get_type()` producing names
//! like `Box_int`). Types are compared by [`TypeId`] equality and monomorphization is keyed by
//! `(DefId, args)` rather than by mangled strings; surface spellings are reconstructed only for
//! diagnostics via [`display::display_name`].

mod compat;
mod def;
mod display;
mod interner;
mod kind;
mod lower;
mod naming;

pub use compat::{assignable, numeric_widen, overload_compatible};
pub use def::{DefInfo, DefKind, DefTable};
pub use display::{display_name, UNKNOWN_TYPE_NAME};
pub use interner::TypeInterner;
pub use kind::{PrimTy, TyKind};
pub use lower::TypeCtx;
pub use naming::{constructor_fn, json_from_json_fn, json_to_json_fn, method_fn, value_size_align};

/// A compact handle to an interned type. Equal ids denote structurally equal types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(pub u32);

/// A compact handle to a nominal declaration (struct/union/enum/function) in a [`DefTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefId(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_deduplicated() {
        let mut i = TypeInterner::new();
        let a = i.array(i.int());
        let b = i.array(i.int());
        assert_eq!(a, b, "identical array types must intern to the same id");
        let c = i.array(i.string());
        assert_ne!(a, c);
    }

    #[test]
    fn nullable_collapses() {
        let mut i = TypeInterner::new();
        let n = i.nullable(i.int());
        let nn = i.nullable(n);
        assert_eq!(n, nn, "T?? collapses to T?");
    }

    #[test]
    fn reference_classification() {
        let mut i = TypeInterner::new();
        assert!(!i.is_reference(i.int()));
        assert!(i.is_reference(i.string()));
        let arr = i.array(i.int());
        assert!(i.is_reference(arr));
        let nullable_arr = i.nullable(arr);
        assert!(i.is_reference(nullable_arr), "nullable wrapper is stripped");
    }

    #[test]
    fn display_renders_generics_with_angle_brackets() {
        let mut defs = DefTable::new();
        let mut i = TypeInterner::new();
        let def = defs.intern(DefKind::Struct, "Box", vec!["T".to_string()]);
        let boxed_int = i.struct_ty(def, vec![i.int()]);
        assert_eq!(display_name(&i, &defs, boxed_int), "Box<int>");
        let arr = i.array(i.int());
        assert_eq!(display_name(&i, &defs, arr), "int[]");
        let opt = i.nullable(i.string());
        assert_eq!(display_name(&i, &defs, opt), "string?");
    }

    #[test]
    fn def_table_dedups_by_name() {
        let mut defs = DefTable::new();
        let a = defs.intern(DefKind::Struct, "Point", vec![]);
        let b = defs.intern(DefKind::Struct, "Point", vec![]);
        assert_eq!(a, b);
        let f = defs.intern(DefKind::Function, "Point", vec![]);
        assert_ne!(
            a, f,
            "different DefKinds with the same name are distinct defs"
        );
    }

    #[test]
    fn assignability_rules() {
        let mut defs = DefTable::new();
        let mut i = TypeInterner::new();

        // Identity and object-top.
        assert!(assignable(&i, i.int(), i.int()));
        assert!(assignable(&i, i.object(), i.string()));

        // Numeric widening is directional.
        let long = i.prim(PrimTy::Long);
        assert!(assignable(&i, long, i.int()));
        assert!(!assignable(&i, i.int(), long));

        // Enum <-> int.
        let color = defs.intern(DefKind::Enum, "Color", vec![]);
        let color_ty = i.enum_ty(color);
        assert!(assignable(&i, color_ty, i.int()));
        assert!(assignable(&i, i.int(), color_ty));

        // Nullable target accepts bare inner and the null literal.
        let opt_int = i.nullable(i.int());
        assert!(assignable(&i, opt_int, i.int()));
        let null_lit = i.nullable(i.void());
        assert!(assignable(&i, opt_int, null_lit));

        // Poison is bidirectionally compatible.
        assert!(assignable(&i, i.int(), i.error()));
        assert!(assignable(&i, i.error(), i.string()));
    }

    #[test]
    fn overload_compatibility_is_loose_on_numerics() {
        let mut i = TypeInterner::new();
        // Narrowing direction is fine for overload viability though not for assignment.
        let long = i.prim(PrimTy::Long);
        assert!(overload_compatible(&i, i.int(), long));
        assert!(overload_compatible(&i, long, i.int()));
        assert!(!overload_compatible(&i, i.int(), i.string()));
    }
}
