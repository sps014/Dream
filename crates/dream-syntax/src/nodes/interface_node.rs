use crate::token::syntax_token::SyntaxToken;
use std::rc::Rc;

/// An `interface` declaration: a named set of method signatures a class can implement. Interfaces
/// declare method signatures only (no instance fields, no default bodies in v1); a class satisfies
/// an interface by providing a matching method for each signature. Interfaces cannot be
/// instantiated; an interface-typed value is a tagged object pointer whose method calls dispatch
/// dynamically through the object's runtime tag (see itable dispatch in codegen).
#[derive(Debug, Clone)]
pub struct InterfaceDeclarationNode<'a> {
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    pub generic_parameters: Option<Vec<SyntaxToken>>,
    /// Bounds on the generic parameters. Empty when unconstrained.
    pub generic_constraints: Vec<crate::nodes::GenericConstraint>,
    /// The interface's method signatures. Each is a body-less [`FunctionNode`] (parsed like an
    /// `extern fun ...;`); only the name/params/return type are meaningful.
    pub methods: Vec<crate::nodes::function::FunctionNode<'a>>,
    /// True when the interface is marked `public`.
    pub is_public: bool,
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
        is_public: bool,
    ) -> Self {
        Self {
            attributes,
            name,
            generic_parameters,
            generic_constraints: Vec::new(),
            methods,
            is_public,
            file_path: None,
        }
    }
}
