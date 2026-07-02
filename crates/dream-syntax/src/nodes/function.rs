use super::statement::StatementNode;
use super::types::Type;
use crate::token::syntax_token::SyntaxToken;
use std::rc::Rc;

/// Represents a function parameter in the AST
#[derive(Debug, Clone)]
pub struct ParameterNode {
    pub name: SyntaxToken,
    pub type_: Type,
    /// An optional default value, restricted to a constant literal (`= 5`, `= "hi"`, `= true`,
    /// `= -1`, `= null`). When present, the parameter may be omitted at a call site and the default
    /// is substituted. `None` for required parameters and all synthesized parameters (e.g. `this`).
    pub default: Option<Type>,
}

impl ParameterNode {
    /// Creates a new required parameter node (no default value).
    pub fn new(name: SyntaxToken, type_: Type) -> ParameterNode {
        ParameterNode {
            name,
            type_,
            default: None,
        }
    }

    /// Creates a parameter node with a constant-literal default value.
    pub fn with_default(name: SyntaxToken, type_: Type, default: Option<Type>) -> ParameterNode {
        ParameterNode {
            name,
            type_,
            default,
        }
    }
}

/// Distinguishes a TypeScript-style property accessor from an ordinary method. A getter
/// (`get name(): T { ... }`) takes no parameters and is invoked by reading `obj.name`; a setter
/// (`set name(value: T) { ... }`) takes one parameter and is invoked by writing `obj.name = v`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessorKind {
    Get,
    Set,
}

/// Contextual keyword introducing a getter accessor.
pub const GET_ACCESSOR: &str = "get";
/// Contextual keyword introducing a setter accessor.
pub const SET_ACCESSOR: &str = "set";

impl AccessorKind {
    /// Classifies a member-leading identifier as an accessor keyword. `get`/`set` are contextual
    /// keywords (they remain ordinary identifiers/method names elsewhere, e.g. the indexer hooks
    /// `get(i)`/`set(i, v)`), so callers must additionally confirm the surrounding accessor shape
    /// (`get <name>(...)`) before treating the member as an accessor.
    pub fn from_keyword(text: &str) -> Option<AccessorKind> {
        match text {
            GET_ACCESSOR => Some(AccessorKind::Get),
            SET_ACCESSOR => Some(AccessorKind::Set),
            _ => None,
        }
    }

    /// The contextual keyword spelling for this accessor kind.
    pub fn keyword(self) -> &'static str {
        match self {
            AccessorKind::Get => GET_ACCESSOR,
            AccessorKind::Set => SET_ACCESSOR,
        }
    }
}

/// Represents a function declaration in the AST
#[derive(Debug, Clone)]
pub struct FunctionNode<'a> {
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    pub generic_parameters: Option<Vec<SyntaxToken>>,
    /// Bounds on the generic parameters (`fun min<T : Comparable<T>>(...)`). Empty when unconstrained.
    pub generic_constraints: Vec<crate::nodes::GenericConstraint>,
    pub return_type: Option<Type>,
    pub parameters: Vec<ParameterNode>,
    pub body: &'a [StatementNode<'a>],
    /// True when the declaration is marked `public`: it is visible to other modules and (for
    /// top-level functions) emitted as a WebAssembly export. Private (the default) symbols are
    /// module-internal.
    pub is_public: bool,
    /// True for `extern fun` declarations: the function has no body and is lowered to a WASM
    /// import instead of a defined function. Used for JS interop.
    pub is_extern: bool,
    /// True for `static fun` methods declared inside a `struct`/`extend` block: the method has no
    /// implicit `this` parameter and is dispatched via `Type.method(...)` instead of `value.method(...)`.
    pub is_static: bool,
    /// True for `async fun` declarations: calling the function eagerly starts a task and yields a
    /// `Future<T>` handle. The body is lowered to a resumable state machine driven by the scheduler.
    pub is_async: bool,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics can report the correct file. `None` for synthesized nodes.
    pub file_path: Option<Rc<str>>,
    /// `Some` when this is a TypeScript-style property accessor (`get`/`set`) rather than an
    /// ordinary method; `name` then holds the property name. `None` for normal methods/functions.
    pub accessor: Option<AccessorKind>,
}

impl<'a> FunctionNode<'a> {
    /// Creates a new function node
    pub fn new(
        attributes: Vec<crate::nodes::AttributeNode>,
        name: SyntaxToken,
        generic_parameters: Option<Vec<SyntaxToken>>,
        return_type: Option<Type>,
        parameters: Vec<ParameterNode>,
        body: &'a [StatementNode<'a>],
        is_public: bool,
    ) -> FunctionNode<'a> {
        FunctionNode {
            attributes,
            name,
            generic_parameters,
            generic_constraints: Vec::new(),
            return_type,
            parameters,
            body,
            is_public,
            is_extern: false,
            is_static: false,
            is_async: false,
            file_path: None,
            accessor: None,
        }
    }
}
