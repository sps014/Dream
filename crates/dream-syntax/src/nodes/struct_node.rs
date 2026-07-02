use crate::nodes::Type;
use crate::token::syntax_token::SyntaxToken;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct StructFieldNode {
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    /// True when the field is marked `public`. Private (the default) fields may only be read or
    /// written from within the declaring type's own methods.
    pub is_public: bool,
    /// The field type's canonical spelling as a token (carries the source position and a flat
    /// display name like `List_JsonValue`). For the structured type (which preserves generic
    /// arguments such as `List<JsonValue>`), use `field_type`.
    pub type_token: SyntaxToken,
    /// The fully parsed field type, preserving generic arguments, arrays, and nullability so
    /// generic field types (e.g. `List<JsonValue>`, `Map<string, V>`) can be instantiated and
    /// have their methods resolved.
    pub field_type: Type,
}

#[derive(Debug, Clone)]
pub struct StructDeclarationNode<'a> {
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    pub generic_parameters: Option<Vec<SyntaxToken>>,
    pub fields: Vec<StructFieldNode>,
    pub methods: Vec<crate::nodes::function::FunctionNode<'a>>,
    /// The interfaces this class declares it implements (`class Cat : Animal, Container<int>`).
    /// Each entry is a (possibly generic) interface type; the class must provide a matching method
    /// for every method of each listed interface (validated during semantic analysis). Empty when
    /// no `:` clause is present.
    pub implements: Vec<Type>,
    /// True when the class is marked `public`: visible to other modules and emitted as a
    /// WebAssembly export. Private (the default) classes are module-internal.
    pub is_public: bool,
    /// True when declared with the `struct` keyword (a value type): stored inline with copy
    /// semantics rather than as a heap-allocated, reference-counted `class`.
    pub is_value: bool,
    /// Source file this declaration came from; set during multi-file merge so semantic
    /// diagnostics can report the correct file. `None` for synthesized nodes.
    pub file_path: Option<Rc<str>>,
}

impl<'a> StructDeclarationNode<'a> {
    pub fn new(
        attributes: Vec<crate::nodes::AttributeNode>,
        name: SyntaxToken,
        generic_parameters: Option<Vec<SyntaxToken>>,
        fields: Vec<StructFieldNode>,
        methods: Vec<crate::nodes::function::FunctionNode<'a>>,
        is_public: bool,
    ) -> Self {
        Self {
            attributes,
            name,
            generic_parameters,
            fields,
            methods,
            implements: Vec::new(),
            is_public,
            is_value: false,
            file_path: None,
        }
    }
}
