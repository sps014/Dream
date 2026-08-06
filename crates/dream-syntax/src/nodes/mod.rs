pub mod expression;
pub mod function;
pub mod interface_node;
pub mod pattern;
pub mod program;
pub mod statement;
pub mod struct_node;
pub mod types;

pub use expression::{
    ExpressionNode, LambdaBody, LambdaNode, SwitchArm, SwitchArmBody, SyntaxBlockNode,
    SyntaxBlockPart,
};
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

/// One constant argument to `@name(...)`. Attribute args are compile-time constants only —
/// string/int/float/double/bool literals, or a dotted enum-member path (`HttpMethod.Get`).
#[derive(Debug, Clone)]
pub enum AttributeArg {
    String(SyntaxToken),
    Int(SyntaxToken),
    Float(SyntaxToken),
    Double(SyntaxToken),
    Bool(SyntaxToken),
    /// Dotted path of identifiers (e.g. `HttpMethod`, `Get`).
    Enum(Vec<SyntaxToken>),
}

impl AttributeArg {
    pub fn position(&self) -> dream_text::text_span::TextSpan {
        match self {
            AttributeArg::String(t)
            | AttributeArg::Int(t)
            | AttributeArg::Float(t)
            | AttributeArg::Double(t)
            | AttributeArg::Bool(t) => t.position,
            AttributeArg::Enum(parts) => parts[0].position,
        }
    }

    /// Source-facing text for diagnostics (keeps quotes on strings).
    pub fn display(&self) -> String {
        match self {
            AttributeArg::String(t)
            | AttributeArg::Int(t)
            | AttributeArg::Float(t)
            | AttributeArg::Double(t)
            | AttributeArg::Bool(t) => t.text.clone(),
            AttributeArg::Enum(parts) => parts
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join("."),
        }
    }

    /// Unquoted string contents, if this is a string literal.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            AttributeArg::String(t) => {
                let s = t.text.as_str();
                if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
                    Some(&s[1..s.len() - 1])
                } else {
                    Some(s)
                }
            }
            _ => None,
        }
    }

    /// Integer literal text (no suffix), if this is an int arg.
    pub fn as_int_text(&self) -> Option<&str> {
        match self {
            AttributeArg::Int(t) => Some(t.text.as_str()),
            _ => None,
        }
    }

    /// Value used by SemanticModel / codegen facades (strings unquoted; enum as dotted path).
    pub fn semantic_value(&self) -> String {
        match self {
            AttributeArg::String(_) => self.as_string().unwrap_or("").to_string(),
            AttributeArg::Int(t)
            | AttributeArg::Float(t)
            | AttributeArg::Double(t)
            | AttributeArg::Bool(t) => t.text.clone(),
            AttributeArg::Enum(_) => self.display(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttributeNode {
    pub name: SyntaxToken,
    pub args: Vec<AttributeArg>,
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
