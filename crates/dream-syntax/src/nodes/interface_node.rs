use crate::nodes::Visibility;
use crate::token::syntax_token::SyntaxToken;
use std::rc::Rc;

/// An `interface` declaration: a named set of methods a class can implement. Methods are usually
/// signature-only, but may supply a *default* body (`fun f() { ... }`) that implementing classes
/// inherit when they omit the method. Interfaces declare no instance fields. A class satisfies an
/// interface by providing a matching method for each signature (defaults excepted). Interfaces
/// cannot be instantiated; an interface-typed value is a tagged object pointer whose method calls
/// dispatch dynamically through the object's runtime tag (see itable dispatch in codegen).
///
/// An interface may extend one or more parent interfaces (`interface Child : Parent + Other`):
/// implementers of the child are subtypes of every parent, and the child's method set includes
/// inherited parent methods (child declarations override same-named parents).
#[derive(Debug, Clone)]
pub struct InterfaceDeclarationNode<'a> {
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    pub generic_parameters: Option<Vec<SyntaxToken>>,
    /// Bounds on the generic parameters. Empty when unconstrained.
    pub generic_constraints: Vec<crate::nodes::GenericConstraint>,
    /// Parent interfaces from `: Parent (+ Parent)*`. Empty when the interface stands alone.
    /// Types are usually `Type::Struct` naming another interface (possibly with generic args).
    pub parents: Vec<crate::nodes::Type>,
    /// The interface's method signatures. Each is a body-less [`FunctionNode`] (parsed like an
    /// `extern fun ...;`); only the name/params/return type are meaningful. Inherited parent
    /// methods are *not* duplicated here — the analyzer flattens the parent closure at
    /// registration / monomorphization time.
    pub methods: Vec<crate::nodes::function::FunctionNode<'a>>,
    /// Accessibility of the interface (`public`/`internal`/private, the default).
    pub visibility: Visibility,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics can report the correct file. `None` for synthesized nodes.
    pub file_path: Option<Rc<str>>,
}

impl<'a> InterfaceDeclarationNode<'a> {
    pub fn new(
        attributes: Vec<crate::nodes::AttributeNode>,
        name: SyntaxToken,
        generic_parameters: Option<Vec<SyntaxToken>>,
        methods: Vec<crate::nodes::function::FunctionNode<'a>>,
        visibility: Visibility,
    ) -> Self {
        Self {
            attributes,
            name,
            generic_parameters,
            generic_constraints: Vec::new(),
            parents: Vec::new(),
            methods,
            visibility,
            file_path: None,
        }
    }
}
