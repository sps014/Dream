use crate::nodes::Type;
use crate::nodes::Visibility;
use crate::token::syntax_token::SyntaxToken;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct StructFieldNode {
    pub attributes: Vec<crate::nodes::AttributeNode>,
    pub name: SyntaxToken,
    /// Accessibility of the field: `public` may be read/written anywhere, `internal` anywhere in
    /// the same declaring module, private (the default) only from the declaring type's own methods.
    pub visibility: Visibility,
    /// True when the field is marked `weak`: it must be `Option<T>` for some reference type `T`,
    /// does not contribute to `T`'s strong reference count, and is automatically reset to `None`
    /// once the referent's strong count reaches zero. Breaks ARC reference cycles without keeping
    /// the referent alive. Mutually exclusive with `is_unowned` (enforced during semantic analysis).
    pub is_weak: bool,
    /// True when the field is marked `unowned`: a plain (non-`Option`) reference-type field that
    /// does not contribute to the referent's strong reference count. Accessing it after the
    /// referent has been freed traps at runtime. Mutually exclusive with `is_weak`.
    pub is_unowned: bool,
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
    /// Bounds on the generic parameters (`class Sorted<T : Comparable<T>>`). Empty when unconstrained.
    pub generic_constraints: Vec<crate::nodes::GenericConstraint>,
    pub fields: Vec<StructFieldNode>,
    pub methods: Vec<crate::nodes::function::FunctionNode<'a>>,
    /// The interfaces this class declares it implements (`class Cat : Animal, Container<int>`).
    /// Each entry is a (possibly generic) interface type; the class must provide a matching method
    /// for every method of each listed interface (validated during semantic analysis). Empty when
    /// no `:` clause is present.
    pub implements: Vec<Type>,
    /// Accessibility of the type: `public` is visible everywhere and emitted as a WebAssembly
    /// export; `internal` is visible anywhere in the same declaring module; private (the default)
    /// is file-scoped.
    pub visibility: Visibility,
    /// True when declared with the `struct` keyword (a value type): stored inline with copy
    /// semantics rather than as a heap-allocated, reference-counted `class`.
    pub is_value: bool,
    /// True when declared `ref struct`: a stack-only value type. Implies `is_value`; additionally,
    /// the analyzer rejects any use that would let an instance escape the current stack frame
    /// (stored in a heap object, used as a generic type argument, closure-captured, or crossing an
    /// `async` boundary) — see `Analyzer::check_ref_struct_escapes`.
    pub is_ref_struct: bool,
    /// True when declared `sealed`: no `extend` block may target this type. Guards the type's method
    /// surface against outside extension (enforced during semantic analysis).
    pub is_sealed: bool,
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
        visibility: Visibility,
    ) -> Self {
        Self {
            attributes,
            name,
            generic_parameters,
            generic_constraints: Vec::new(),
            fields,
            methods,
            implements: Vec::new(),
            visibility,
            is_value: false,
            is_ref_struct: false,
            is_sealed: false,
            file_path: None,
        }
    }
}
