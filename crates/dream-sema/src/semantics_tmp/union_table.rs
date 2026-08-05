//! Layout information for discriminated unions (data-carrying enums). A union value is a
//! reference-counted heap block laid out like a struct but with a leading `i32` discriminant at
//! offset 0; each variant's payload fields are placed at fixed offsets starting after it. Payloads
//! of different variants overlap (the block is sized to the largest variant), so the active
//! variant is identified solely by the discriminant.

use crate::syntax::nodes::Type;
use indexmap::IndexMap;

/// Byte offset of the discriminant word; payload fields start immediately after it.
pub const DISCRIMINANT_SIZE: usize = 4;

/// A single payload field of a union variant.
#[derive(Debug, Clone)]
pub struct UnionFieldInfo {
    pub name: String,
    pub type_: Type,
    /// Byte offset from the start of the block (always >= DISCRIMINANT_SIZE).
    pub offset: usize,
}

/// A single variant of a discriminated union.
#[derive(Debug, Clone)]
pub struct UnionVariantInfo {
    pub name: String,
    /// The discriminant stored at offset 0 to identify this variant at runtime.
    pub discriminant: i32,
    pub fields: Vec<UnionFieldInfo>,
}

/// The resolved layout of a (monomorphized) discriminated union.
#[derive(Debug, Clone)]
pub struct UnionInfo {
    pub name: String,
    pub variants: Vec<UnionVariantInfo>,
    /// Total heap-block size: discriminant + the largest variant's payload (word-aligned).
    pub size: usize,
}

impl UnionInfo {
    pub fn variant(&self, name: &str) -> Option<&UnionVariantInfo> {
        self.variants.iter().find(|v| v.name == name)
    }
}

/// Registered (monomorphized) unions: name -> layout. Insertion-ordered so the union protocol
/// defaults and release code emit in a deterministic (registration) order.
pub type UnionTable = IndexMap<String, UnionInfo>;
