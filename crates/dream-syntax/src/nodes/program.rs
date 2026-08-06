use super::expression::ExpressionNode;
use super::function::FunctionNode;
use super::interface_node::InterfaceDeclarationNode;
use super::struct_node::{StructDeclarationNode, StructFieldNode};
use super::types::Type;
use crate::nodes::Visibility;
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
    /// Accessibility of the variable (`public`/`internal`/private, the default).
    pub visibility: Visibility,
    /// `static` pins the variable to file/module-internal linkage (it can never be `public`).
    pub is_static: bool,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics can report the correct file. `None` for synthesized nodes.
    pub file_path: Option<Rc<str>>,
}

/// Represents an import declaration in the AST. Two forms share this node, told apart by
/// `alias`:
/// - `import a.b.c;` (`alias: None`) — resolves `module_name` (`a/b/c`) against the filesystem and
///   pulls in the whole file's public declarations, exactly as before `module`/`as` existed.
/// - `import a.b.c as x;` (`alias: Some(x)`) — resolves `module_name` (`a.b.c`, dot-separated, not
///   slash-separated: this form names a *declared* `module` namespace, not a file path) against
///   already-loaded modules, binding just the item `c` into the importing file's scope as `x`.
#[derive(Debug, Clone)]
pub struct ImportNode {
    pub module_name: SyntaxToken,
    pub alias: Option<SyntaxToken>,
}

impl ImportNode {
    /// Creates a new plain (whole-file) import node.
    pub fn new(module_name: SyntaxToken) -> ImportNode {
        ImportNode {
            module_name,
            alias: None,
        }
    }

    /// Creates an aliased, module-qualified-item import node (`import a.b.c as x;`).
    pub fn with_alias(module_name: SyntaxToken, alias: SyntaxToken) -> ImportNode {
        ImportNode {
            module_name,
            alias: Some(alias),
        }
    }
}

/// A file-scoped `module a.b.c;` declaration: names the current file's own module, independent of
/// its directory location. At most one per file, and it must be the first item in the file
/// (enforced by the parser). Files that declare none belong to the implicit, unnamed root module.
#[derive(Debug, Clone)]
pub struct ModuleDeclNode {
    /// Attributes on the module declaration.
    pub attributes: Vec<crate::nodes::AttributeNode>,
    /// The dot-joined module path (e.g. `"utils.math"`), stored verbatim (not slash-joined like
    /// [`ImportNode::module_name`], since this never touches the filesystem).
    pub path: SyntaxToken,
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
pub struct EnumDeclarationNode<'a> {
    /// Leading attributes (`@json`, ...). Carried so derives like `@json` can target unions.
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    /// Generic type parameters for a generic discriminated union (`enum Option<T> { ... }`).
    pub generic_parameters: Option<Vec<SyntaxToken>>,
    /// Bounds on the generic parameters (`enum Box<T : Comparable<T>>`). Empty when unconstrained.
    pub generic_constraints: Vec<crate::nodes::GenericConstraint>,
    pub variants: Vec<EnumVariantNode>,
    /// Instance/static methods declared in the enum body (same lowering as class/`extend` methods).
    pub methods: Vec<crate::nodes::function::FunctionNode<'a>>,
    /// True when declared `sealed`: no `extend` block may target this enum (enforced in analysis).
    pub is_sealed: bool,
    /// Accessibility of the enum (`public`/`internal`/private, the default).
    pub visibility: Visibility,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics and cross-file visibility checks can identify the declaring module. `None`
    /// for synthesized nodes.
    pub file_path: Option<Rc<str>>,
}

impl<'a> EnumDeclarationNode<'a> {
    pub fn new(
        attributes: Vec<crate::nodes::AttributeNode>,
        name: SyntaxToken,
        generic_parameters: Option<Vec<SyntaxToken>>,
        variants: Vec<EnumVariantNode>,
    ) -> EnumDeclarationNode<'a> {
        EnumDeclarationNode {
            attributes,
            name,
            generic_parameters,
            generic_constraints: Vec::new(),
            variants,
            methods: Vec::new(),
            is_sealed: false,
            visibility: Visibility::Private,
            file_path: None,
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
    /// Interfaces this `extend` block declares its target satisfies (`extend int : Comparable<int>`).
    /// Validated in analysis, which records the implementation so the target participates in
    /// interface dispatch and generic constraints. Empty for a plain `extend`.
    pub implements: Vec<Type>,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics can report the correct file. `None` for synthesized nodes.
    pub file_path: Option<Rc<str>>,
    /// True for compiler-synthesized `extend` blocks (interface default methods, `@json`
    /// converters) rather than user source. These bypass the `sealed` restriction, so a sealed
    /// type may still implement interfaces with defaults or derive `@json`.
    pub is_synthesized: bool,
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
            implements: Vec::new(),
            file_path: None,
            is_synthesized: false,
        }
    }
}

/// Represents the root program node in the AST
#[derive(Debug, Clone)]
pub struct ProgramNode<'a> {
    /// This file's own `module a.b.c;` declaration, if any. `None` places the file in the
    /// implicit, unnamed root module.
    pub module: Option<ModuleDeclNode>,
    pub imports: Vec<ImportNode>,
    pub structs: Vec<StructDeclarationNode<'a>>,
    pub interfaces: Vec<InterfaceDeclarationNode<'a>>,
    pub functions: Vec<FunctionNode<'a>>,
    pub enums: Vec<EnumDeclarationNode<'a>>,
    pub extends: Vec<ExtendNode<'a>>,
    /// Top-level `let`/`const` variables declared outside any class or function.
    pub globals: Vec<GlobalVariableNode<'a>>,
}

impl<'a> ProgramNode<'a> {
    /// Creates a new program node
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        imports: Vec<ImportNode>,
        structs: Vec<StructDeclarationNode<'a>>,
        interfaces: Vec<InterfaceDeclarationNode<'a>>,
        functions: Vec<FunctionNode<'a>>,
        enums: Vec<EnumDeclarationNode<'a>>,
        extends: Vec<ExtendNode<'a>>,
        globals: Vec<GlobalVariableNode<'a>>,
    ) -> ProgramNode<'a> {
        ProgramNode {
            module: None,
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
