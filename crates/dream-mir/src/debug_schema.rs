//! The runtime-type schema shared by both sides of the `.dbg.json` debug source map: the writer
//! ([`crate::mir::emit::debug_map`], built into every configuration of the compiler) and the reader
//! ([`crate::execution::debugger::sourcemap`], native-only — it drives the wasmtime-based debug
//! adapter). Both walked-and-decoded the same structural description of a runtime type (struct
//! fields, union variants, array element/stride, scalar encoding) from two independently
//! hand-maintained copies; this module is the single definition they now share.
//!
//! Deliberately dependency-free (no `serde`): the writer side must keep building in every build
//! configuration, including ones without the `native` feature (and its `serde_json` dependency)
//! enabled. Each side still owns its own serialization: the writer hand-renders compact JSON
//! (see `debug_map::type_to_json`) and the reader parses `serde_json::Value` (see
//! `sourcemap::parse_type`) — only the schema these mirror is unified here.

/// A scalar (non-aggregate, non-reference) value's runtime encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Int,
    UInt,
    Byte,
    Bool,
    Char,
    Long,
    ULong,
    Float,
    Double,
}

impl ScalarKind {
    /// The JSON `"scalar"` tag this kind round-trips through.
    pub fn tag(self) -> &'static str {
        match self {
            ScalarKind::Int => "int",
            ScalarKind::UInt => "uint",
            ScalarKind::Byte => "byte",
            ScalarKind::Bool => "bool",
            ScalarKind::Char => "char",
            ScalarKind::Long => "long",
            ScalarKind::ULong => "ulong",
            ScalarKind::Float => "float",
            ScalarKind::Double => "double",
        }
    }

    /// Byte width of the scalar in linear memory.
    pub fn width(self) -> u32 {
        match self {
            ScalarKind::Byte | ScalarKind::Bool => 1,
            ScalarKind::Long | ScalarKind::ULong | ScalarKind::Double => 8,
            _ => 4,
        }
    }
}

/// One field of a struct or a union variant payload.
#[derive(Debug, Clone)]
pub struct FieldDesc {
    pub name: String,
    pub offset: u32,
    /// Index into the owning [`TypeDesc`] table.
    pub type_id: u32,
}

/// One variant of a discriminated union.
#[derive(Debug, Clone)]
pub struct VariantDesc {
    pub name: String,
    pub discriminant: i32,
    pub fields: Vec<FieldDesc>,
}

/// A structural description of a runtime type, sufficient to walk memory and decode a live value.
/// Recursive: aggregates reference their component types by index into the owning type table.
#[derive(Debug, Clone)]
pub enum TypeDesc {
    Scalar(ScalarKind),
    /// A `string`: pointer to `[len:i32][utf8...]`.
    Str,
    /// C-style enum (an `i32` discriminant).
    Enum,
    Struct {
        name: String,
        /// True for value (inline) structs; false for reference (heap) structs.
        value: bool,
        fields: Vec<FieldDesc>,
    },
    Union {
        name: String,
        value: bool,
        variants: Vec<VariantDesc>,
    },
    Array {
        elem: u32,
        /// Byte stride between consecutive elements.
        stride: u32,
    },
    /// An opaque reference (interface/object/function value, or a type with no known layout): shown
    /// as an address.
    Ref,
}
