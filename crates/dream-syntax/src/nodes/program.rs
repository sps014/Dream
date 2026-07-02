use super::expression::ExpressionNode;
use super::function::FunctionNode;
use super::interface_node::InterfaceDeclarationNode;
use super::struct_node::{StructDeclarationNode, StructFieldNode};
use super::types::Type;
use crate::token::syntax_token::SyntaxToken;
use std::rc::Rc;

/// A top-level variable declaration: `let`/`const` written outside any class or function. The
/// initializer is an arbitrary expression evaluated once, in declaration order, by the generated
/// module-init function that runs before `main`.
#[derive(Debug, Clone)]
pub struct GlobalVariableNode<'a> {
    pub name: SyntaxToken,
    /// The explicit type annotation, if written (`let x: int = ...`). When absent the type is
    /// inferred from the initializer.
    pub declared_type: Option<Type>,
    pub initializer: ExpressionNode<'a>,
    /// `const` declarations may not be reassigned after initialization.
    pub is_const: bool,
    /// `public` exposes the variable to other modules; private (the default) is module-internal.
    pub is_public: bool,
    /// `static` pins the variable to file/module-internal linkage (it can never be `public`).
    pub is_static: bool,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics can report the correct file. `None` for synthesized nodes.
    pub file_path: Option<Rc<str>>,
}

/// Represents an import declaration in the AST
#[derive(Debug, Clone)]
pub struct ImportNode {
    pub module_name: SyntaxToken,
}

impl ImportNode {
    /// Creates a new import node
    pub fn new(module_name: SyntaxToken) -> ImportNode {
        ImportNode { module_name }
    }
}

/// A single variant of an `enum`. A variant with no `fields` is either a plain C-style member
/// (`Red`, `Green = 5`) or a unit variant of a discriminated union (`None`, `Empty`). A variant
/// with one or more `fields` carries a typed payload (`Circle(radius: float)`), which turns the
/// whole enum into a heap-backed discriminated union.
#[derive(Debug, Clone)]
pub struct EnumVariantNode {
    pub name: SyntaxToken,
    /// The variant's payload fields, in declaration order. Empty for unit / C-style members.
    pub fields: Vec<StructFieldNode>,
    /// The variant's integer value. For C-style enums this is the member value (explicit or
    /// auto-assigned, C-style); for discriminated unions this is the variant's discriminant.
    pub value: i32,
}

/// Represents an enum declaration. Two flavours share this node:
/// - C-style integer enums: `enum Color { Red, Green = 5, Blue }` (all variants payload-less).
/// - Discriminated unions (Rust-style): `enum Shape { Circle(radius: float), Empty }` and
///   generic `enum Option<T> { Some(value: T), None }` (at least one variant carries a payload).
#[derive(Debug, Clone)]
pub struct EnumDeclarationNode {
    /// Leading attributes (`@json`, ...). Carried so derives like `@json` can target unions.
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    /// Generic type parameters for a generic discriminated union (`enum Option<T> { ... }`).
    pub generic_parameters: Option<Vec<SyntaxToken>>,
    pub variants: Vec<EnumVariantNode>,
}

impl EnumDeclarationNode {
    pub fn new(
        attributes: Vec<crate::nodes::AttributeNode>,
        name: SyntaxToken,
        generic_parameters: Option<Vec<SyntaxToken>>,
        variants: Vec<EnumVariantNode>,
    ) -> EnumDeclarationNode {
        EnumDeclarationNode {
            attributes,
            name,
            generic_parameters,
            variants,
        }
    }

    /// True when any variant carries a payload, i.e. this enum is a discriminated union rather
    /// than a plain C-style integer enum.
    pub fn is_data_enum(&self) -> bool {
        self.variants.iter().any(|v| !v.fields.is_empty())
    }
}

/// Represents an `extend Type { ... }` block: a set of methods attached to an existing
/// type (a primitive, `object`, or a struct) without changing that type's runtime
/// representation. Methods are lowered exactly like struct methods (`{target}_{method}`
/// with an implicit `this` parameter).
#[derive(Debug, Clone)]
pub struct ExtendNode<'a> {
    /// The canonical name of the type being extended (e.g. `int`, `string`, `Point`).
    pub target: SyntaxToken,
    pub generic_parameters: Option<Vec<SyntaxToken>>,
    /// Bounds on the generic parameters (`extend List<T : Comparable<T>> { ... }`): the extension's
    /// methods only attach to a monomorphized target whose argument satisfies them. Empty when
    /// unconstrained.
    pub generic_constraints: Vec<crate::nodes::GenericConstraint>,
    pub methods: Vec<FunctionNode<'a>>,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics can report the correct file. `None` for synthesized nodes.
    pub file_path: Option<Rc<str>>,
}

impl<'a> ExtendNode<'a> {
    pub fn new(
        target: SyntaxToken,
        generic_parameters: Option<Vec<SyntaxToken>>,
        methods: Vec<FunctionNode<'a>>,
    ) -> ExtendNode<'a> {
        ExtendNode {
            target,
            generic_parameters,
            generic_constraints: Vec::new(),
            methods,
            file_path: None,
        }
    }
}

/// Represents the root program node in the AST
#[derive(Debug, Clone)]
pub struct ProgramNode<'a> {
    pub imports: Vec<ImportNode>,
    pub structs: Vec<StructDeclarationNode<'a>>,
    pub interfaces: Vec<InterfaceDeclarationNode<'a>>,
    pub functions: Vec<FunctionNode<'a>>,
    pub enums: Vec<EnumDeclarationNode>,
    pub extends: Vec<ExtendNode<'a>>,
    /// Top-level `let`/`const` variables declared outside any class or function.
    pub globals: Vec<GlobalVariableNode<'a>>,
}

impl<'a> ProgramNode<'a> {
    /// Creates a new program node
    pub fn new(
        imports: Vec<ImportNode>,
        structs: Vec<StructDeclarationNode<'a>>,
        interfaces: Vec<InterfaceDeclarationNode<'a>>,
        functions: Vec<FunctionNode<'a>>,
        enums: Vec<EnumDeclarationNode>,
        extends: Vec<ExtendNode<'a>>,
        globals: Vec<GlobalVariableNode<'a>>,
    ) -> ProgramNode<'a> {
        ProgramNode {
            imports,
            structs,
            interfaces,
            functions,
            enums,
            extends,
            globals,
        }
    }
}
