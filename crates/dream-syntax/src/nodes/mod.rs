pub mod expression;
pub mod function;
pub mod interface_node;
pub mod pattern;
pub mod program;
pub mod statement;
pub mod struct_node;
pub mod types;

pub use expression::{ExpressionNode, LambdaBody, LambdaNode, SwitchArm, SwitchArmBody};
pub use function::{FunctionNode, ParameterNode};
pub use interface_node::InterfaceDeclarationNode;
pub use pattern::PatternNode;
pub use program::{
    EnumDeclarationNode, EnumVariantNode, ExtendNode, GlobalVariableNode, ImportNode,
    ModuleDeclNode, ProgramNode,
};
pub use statement::StatementNode;
pub use struct_node::{StructDeclarationNode, StructFieldNode};
pub use types::Type;

use crate::token::syntax_token::SyntaxToken;

/// Accessibility of a top-level declaration (axis 1: file/module visibility) or a class member
/// (axis 2: member visibility). Replaces a plain `is_public: bool` with a third, module-scoped
/// level: `Internal` sits strictly between the file/class-private default and `Public`, visible
/// anywhere in the same declaring module (a `module a.b;` namespace, or the shared unnamed root
/// module for files that declare none) but not from a different module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    /// File-private (axis 1) or class-private (axis 2): the default when no modifier is written.
    #[default]
    Private,
    /// Visible anywhere in the same module, not outside it.
    Internal,
    /// Visible everywhere.
    Public,
}

impl Visibility {
    /// True for `Public` — the only level that crosses module boundaries unconditionally. Named to
    /// read naturally at existing `if decl.is_public()` call sites that predate `Internal`.
    pub fn is_public(self) -> bool {
        matches!(self, Visibility::Public)
    }

    /// True for `Internal` or `Public` — i.e. reachable from at least the declaring module.
    pub fn is_at_least_internal(self) -> bool {
        !matches!(self, Visibility::Private)
    }
}

#[derive(Debug, Clone)]
pub struct AttributeNode {
    pub name: SyntaxToken,
    pub args: Vec<SyntaxToken>,
}

/// A *kind* bound on a generic parameter (C#-aligned): `T : struct` requires a non-nullable value
/// type (a `struct` or a non-`string` primitive) that *may* still contain reference-typed fields;
/// `T : unmanaged` requires a *blittable* value type (recursively only value fields, no inner heap
/// pointers - a strict subset of `struct`); `T : class` requires a reference type. Orthogonal to
/// the interface `bounds` and combinable with them via `+` (e.g. `T : unmanaged + Comparable<T>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    Struct,
    Unmanaged,
    Class,
}

/// A bound on a generic type parameter (`T : Comparable<T>` or `T : Equatable<T> + Comparable<T>`).
/// The bare parameter name is still carried by the declaration's `generic_parameters`; this records
/// the interface types the concrete argument must implement. Each generic declaration (class/struct,
/// interface, function, `extend`) carries a `Vec<GenericConstraint>`, empty when no bounds are given.
#[derive(Debug, Clone)]
pub struct GenericConstraint {
    /// The constrained type parameter (e.g. `T`), matching a name in `generic_parameters`.
    pub param: SyntaxToken,
    /// The interfaces `param` must implement; at least one when a `:` clause is present.
    pub bounds: Vec<Type>,
    /// Kind constraints (`struct`/`class`) parsed from the same `:`-clause, e.g. `T : struct`.
    pub kinds: Vec<ConstraintKind>,
}
