pub mod expression;
pub mod function;
pub mod interface_node;
pub mod pattern;
pub mod program;
pub mod statement;
pub mod struct_node;
pub mod types;

pub use expression::{ExpressionNode, SwitchArm, SwitchArmBody};
pub use function::{FunctionNode, ParameterNode};
pub use interface_node::InterfaceDeclarationNode;
pub use pattern::PatternNode;
pub use program::{
    EnumDeclarationNode, EnumVariantNode, ExtendNode, GlobalVariableNode, ImportNode, ProgramNode,
};
pub use statement::StatementNode;
pub use struct_node::{StructDeclarationNode, StructFieldNode};
pub use types::Type;

use crate::token::syntax_token::SyntaxToken;

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
