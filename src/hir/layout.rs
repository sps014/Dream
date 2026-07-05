//! Memory layout for nominal types: the byte offset, size, and type of each field, which the backend
//! needs to lower `obj.field` / `array[i]` access to concrete loads and stores.
//!
//! Offsets are computed here (not borrowed from the legacy `StructInfo`) with a single, internally
//! consistent size rule ([`scalar_size`]), so the layout and the store widths the emitter picks
//! always agree. Fields are kept in **declaration order**, which coincides with offset order (a
//! struct lays its fields out sequentially), so the resolved field index used in
//! [`super::HPlace::Field`] indexes straight into [`TypeLayout::fields`].

use crate::types::{TyKind, TypeId, TypeInterner};
use indexmap::IndexMap;

/// The in-memory size and alignment (bytes) of a scalar/reference value of `ty`. Reference types
/// (string, array, `class`, union, object), enums, and function values are `i32` pointers/indices
/// (4 bytes). A value (`struct`) type is stored *inline* and occupies its full footprint, recorded
/// on the interner once layouts are computed.
pub fn scalar_size(interner: &TypeInterner, ty: TypeId) -> (u32, u32) {
    // Value structs are stored inline: their footprint is their computed layout size/align.
    if let Some(sz) = interner.value_layout(ty) {
        return sz;
    }
    match interner.kind(interner.strip_nullable(ty)) {
        // Delegates to `PrimTy::size_align` (see there) so this agrees byte-for-byte with the
        // string-keyed `crate::types::naming::value_size_align` used by the legacy struct tables.
        TyKind::Prim(p) => p.size_align(),
        _ => (4, 4),
    }
}

/// One field's position, type, and source name within a struct. The name is carried so the backend
/// can synthesize a default `to_string` (`Point { x: 1, y: 2 }`) without re-consulting the analyzer.
#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub offset: u32,
    pub ty: TypeId,
    pub name: String,
}

/// The full layout of one nominal type.
#[derive(Debug, Clone, Default)]
pub struct TypeLayout {
    /// The type's source display name (e.g. `Point`), used by the default `to_string`.
    pub name: String,
    /// Fields in declaration (== offset) order.
    pub fields: Vec<FieldLayout>,
    /// Total allocated size in bytes (data only; the allocator adds its own header).
    pub size: u32,
}

impl TypeLayout {
    /// Builds a layout from a struct's `(field name, field type)` pairs in declaration order,
    /// assigning aligned offsets. `name` is the struct's display name.
    pub fn from_fields(
        interner: &TypeInterner,
        name: impl Into<String>,
        field_defs: impl IntoIterator<Item = (String, TypeId)>,
    ) -> Self {
        let mut offset = 0u32;
        let mut max_align = 4u32;
        let mut fields = Vec::new();
        for (field_name, ty) in field_defs {
            let (size, align) = scalar_size(interner, ty);
            offset = align_up(offset, align);
            fields.push(FieldLayout {
                offset,
                ty,
                name: field_name,
            });
            offset += size;
            max_align = max_align.max(align);
        }
        TypeLayout {
            name: name.into(),
            fields,
            size: align_up(offset, max_align),
        }
    }
}

fn align_up(offset: u32, align: u32) -> u32 {
    let rem = offset % align;
    if rem == 0 {
        offset
    } else {
        offset + (align - rem)
    }
}

/// The layout of one variant of a discriminated union: its discriminant plus its payload fields.
/// Payload offsets are `>= 4` (a union block leads with an `i32` discriminant at offset 0); the
/// vector index matches [`super::HExprKind::UnionNew::variant`].
#[derive(Debug, Clone)]
pub struct UnionVariant {
    /// The variant's source name (e.g. `Some`), used as its `to_string` label.
    pub name: String,
    /// The value written to the discriminant word (offset 0) to identify this variant.
    pub discriminant: i32,
    /// Payload fields in declaration order, at their fixed block offsets.
    pub fields: Vec<FieldLayout>,
}

/// The layout of a discriminated union. Every variant shares one heap block sized to the largest
/// variant, so any variant fits and the discriminant alone identifies the active one.
#[derive(Debug, Clone, Default)]
pub struct UnionLayout {
    /// The union's source display name, used to name its generated `$<Union>_to_string`.
    pub name: String,
    pub variants: Vec<UnionVariant>,
    /// Total allocated block size (discriminant + largest payload).
    pub size: u32,
}

/// Layouts of all nominal types, keyed by the **interned type id** of the (fully monomorphized)
/// type — so `Box<int>` and `Box<string>`, which share a base `DefId` but differ in field widths,
/// get distinct layouts. Lookup-only (never iterated for emission), so iteration order does not
/// affect codegen determinism.
#[derive(Debug, Clone, Default)]
pub struct LayoutTable {
    pub structs: IndexMap<TypeId, TypeLayout>,
    pub unions: IndexMap<TypeId, UnionLayout>,
}

impl LayoutTable {
    pub fn get(&self, ty: TypeId) -> Option<&TypeLayout> {
        self.structs.get(&ty)
    }

    pub fn insert(&mut self, ty: TypeId, layout: TypeLayout) {
        self.structs.insert(ty, layout);
    }

    pub fn union(&self, ty: TypeId) -> Option<&UnionLayout> {
        self.unions.get(&ty)
    }

    pub fn insert_union(&mut self, ty: TypeId, layout: UnionLayout) {
        self.unions.insert(ty, layout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PrimTy;

    #[test]
    fn packs_and_aligns_fields() {
        let mut i = TypeInterner::new();
        let dbl = i.prim(PrimTy::Double);
        let by = i.prim(PrimTy::Byte);
        let int = i.int();
        // byte(1) @0, then double needs 8-align -> @8, then int @16; size aligns to 8 -> 24.
        let l = TypeLayout::from_fields(
            &i,
            "T",
            [("b".into(), by), ("d".into(), dbl), ("n".into(), int)],
        );
        assert_eq!(l.fields[0].offset, 0);
        assert_eq!(l.fields[1].offset, 8);
        assert_eq!(l.fields[2].offset, 16);
        assert_eq!(l.size, 24);
    }
}
