use dream_syntax::nodes::struct_node::StructDeclarationNode;
use dream_syntax::nodes::{Type, Visibility};
use dream_types::value_size_align;
use indexmap::IndexMap;

#[derive(Debug, Clone)]
pub struct StructFieldInfo {
    pub type_: Type,
    pub offset: usize,
    /// Accessibility of the field. Private (default) fields may only be accessed from within the
    /// declaring type's own methods; `internal` fields from anywhere in the same module.
    pub visibility: Visibility,
    /// True when declared `weak`: an `Option<T>` field that does not hold a strong reference to
    /// its referent and is excluded from the reference-cycle graph.
    pub is_weak: bool,
    /// True when declared `unowned`: a plain reference-type field that does not hold a strong
    /// reference to its referent and is excluded from the reference-cycle graph.
    pub is_unowned: bool,
}

impl StructFieldInfo {
    /// True when this field is excluded from ARC strong-reference bookkeeping (`weak` or
    /// `unowned`), and therefore does not contribute an edge to the reference-cycle graph.
    pub fn is_non_owning(&self) -> bool {
        self.is_weak || self.is_unowned
    }
}

#[derive(Debug, Clone)]
pub struct StructInfo {
    pub name: String,
    /// Insertion-ordered (declaration order) so field-release emission is deterministic. Field
    /// emission that must follow byte-offset order sorts these by their recorded `offset`.
    pub fields: IndexMap<String, StructFieldInfo>,
    pub size: usize,
    pub visibility: Visibility,
    /// True for `struct` (value) types: stored inline with copy semantics, not heap-allocated and
    /// reference-counted. Unions are always reference types (`false`).
    pub is_value: bool,
    /// Source file this type was declared in, for file/module-level visibility: a non-public type
    /// is only referenceable from its own file. `None` for synthesized types (always visible).
    pub file_path: Option<std::rc::Rc<str>>,
}

#[derive(Debug, Clone)]
pub struct StructTable {
    /// Insertion-ordered (registration order) so codegen iterates types deterministically.
    pub structs: IndexMap<String, StructInfo>,
}

impl Default for StructTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StructTable {
    pub fn new() -> Self {
        Self {
            structs: IndexMap::new(),
        }
    }

    pub fn add_struct(&mut self, struct_decl: &StructDeclarationNode<'_>) -> Result<(), String> {
        let name = struct_decl.name.text.clone();
        if self.structs.contains_key(&name) {
            return Err(format!("Struct '{}' is already defined", name));
        }

        let mut fields = IndexMap::new();
        let mut current_offset = 0;

        for field in &struct_decl.fields {
            let field_name = field.name.text.clone();
            if fields.contains_key(&field_name) {
                return Err(format!(
                    "Field '{}' is already defined in class '{}'",
                    field_name, name
                ));
            }

            // Use the structured type parsed by the parser, which preserves generic arguments
            // (e.g. `List<JsonValue>`, `Map<string, V>`) that the flat token text would lose.
            let field_type = field.field_type.clone();

            let (size, alignment) = value_size_align(field_type.get_type().as_str());

            // Align current_offset
            let remainder = current_offset % alignment;
            if remainder != 0 {
                current_offset += alignment - remainder;
            }

            fields.insert(
                field_name,
                StructFieldInfo {
                    type_: field_type,
                    offset: current_offset,
                    visibility: field.visibility,
                    is_weak: field.is_weak,
                    is_unowned: field.is_unowned,
                },
            );
            current_offset += size;
        }

        // Align total size to the largest alignment (usually 8 if double is present, else 4)
        let max_alignment = fields
            .values()
            .map(|f| value_size_align(f.type_.get_type().as_str()).1)
            .max()
            .unwrap_or(4);

        let remainder = current_offset % max_alignment;
        if remainder != 0 {
            current_offset += max_alignment - remainder;
        }

        self.structs.insert(
            name.clone(),
            StructInfo {
                name,
                fields,
                size: current_offset,
                visibility: struct_decl.visibility,
                is_value: struct_decl.is_value,
                file_path: struct_decl.file_path.clone(),
            },
        );

        Ok(())
    }

    /// Registers a discriminated union under `name` as a heap reference type. Unions carry no
    /// flat field map (their payload layout is variant-dependent and lives in the union table),
    /// but they still need an entry here so they receive a runtime type tag, count as a reference
    /// type, and get a (discriminant-aware) `$release_*` helper generated.
    pub fn add_union(
        &mut self,
        name: &str,
        size: usize,
        visibility: Visibility,
        file_path: Option<std::rc::Rc<str>>,
    ) -> Result<(), String> {
        if self.structs.contains_key(name) {
            return Err(format!("Type '{}' is already defined", name));
        }
        self.structs.insert(
            name.to_string(),
            StructInfo {
                name: name.to_string(),
                fields: IndexMap::new(),
                size,
                visibility,
                is_value: false,
                file_path,
            },
        );
        Ok(())
    }

    pub fn get_struct(&self, name: &str) -> Option<&StructInfo> {
        self.structs.get(name)
    }
}
