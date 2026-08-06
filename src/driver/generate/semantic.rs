//! Declaration-level SemanticModel: TypeSymbol / Symbol facades (no DefId exposed).

use dream_syntax::nodes::struct_node::StructDeclarationNode;
use dream_syntax::nodes::{
    AttributeNode, EnumDeclarationNode, FunctionNode, InterfaceDeclarationNode, Type,
};
use indexmap::IndexMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Prim,
    Class,
    Struct,
    RefStruct,
    Enum,
    Union,
    Interface,
    Array,
    Func,
    Js,
    Object,
    Void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Type,
    Field,
    Method,
    Constructor,
    Param,
    Local,
    Variant,
    EnumMember,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

impl From<dream_syntax::nodes::Visibility> for Visibility {
    fn from(v: dream_syntax::nodes::Visibility) -> Self {
        if v.is_public() {
            Visibility::Public
        } else {
            Visibility::Private
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttrInfo {
    pub name: String,
    pub args: Vec<String>,
}

fn attrs_from(nodes: &[AttributeNode]) -> Vec<AttrInfo> {
    nodes
        .iter()
        .map(|a| AttrInfo {
            name: a.name.text.clone(),
            args: a
                .args
                .iter()
                .map(|t| t.text.trim_matches('"').to_string())
                .collect(),
        })
        .collect()
}

fn type_display(t: &Type) -> String {
    match t {
        Type::Integer(_) => "int".into(),
        Type::UInt(_) => "uint".into(),
        Type::Long(_) => "long".into(),
        Type::ULong(_) => "ulong".into(),
        Type::Byte(_) => "byte".into(),
        Type::Float(_) => "float".into(),
        Type::Double(_) => "double".into(),
        Type::Boolean(_) => "bool".into(),
        Type::Char(_) => "char".into(),
        Type::String(_) => "string".into(),
        Type::Void => "void".into(),
        Type::Object(_) => "object".into(),
        Type::Array(e) => format!("{}[]", type_display(e)),
        Type::Struct(tok, args) => {
            if tok.text == "js" {
                return "js".into();
            }
            if let Some(args) = args {
                let inner = args
                    .iter()
                    .map(type_display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", tok.text, inner)
            } else {
                tok.text.clone()
            }
        }
        Type::Function(params, ret) => {
            let ps = params
                .iter()
                .map(type_display)
                .collect::<Vec<_>>()
                .join(", ");
            format!("fun({}): {}", ps, type_display(ret))
        }
        Type::Generic(name) => name.clone(),
        Type::GenericFunctionItem(name) => name.clone(),
        Type::Unknown => "unknown".into(),
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub type_name: String,
    pub visibility: Visibility,
    pub declaring_type: Option<String>,
    pub is_static: bool,
    pub is_async: bool,
    pub is_ref: bool,
    pub is_variadic: bool,
    pub has_default: bool,
    pub is_weak: bool,
    pub is_unowned: bool,
    pub is_unsafe: bool,
    pub is_shared: bool,
    pub attributes: Vec<AttrInfo>,
    pub parameters: Vec<Symbol>,
    pub payload_fields: Vec<Symbol>,
}

impl Symbol {
    pub fn has_attribute(&self, name: &str) -> bool {
        self.attributes.iter().any(|a| a.name == name)
    }

    pub fn attribute_string(&self, name: &str) -> String {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| a.args.first().cloned())
            .unwrap_or_default()
    }

    pub fn attribute_args(&self, name: &str) -> Vec<String> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.args.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct TypeSymbol {
    pub name: String,
    pub display_name: String,
    pub kind: TypeKind,
    pub is_generic: bool,
    pub attributes: Vec<AttrInfo>,
    pub fields: Vec<Symbol>,
    pub methods: Vec<Symbol>,
    pub constructors: Vec<Symbol>,
    pub variants: Vec<Symbol>,
    pub enum_members: Vec<Symbol>,
    pub generic_param_names: Vec<String>,
}

impl TypeSymbol {
    pub fn has_attribute(&self, name: &str) -> bool {
        self.attributes.iter().any(|a| a.name == name)
    }

    pub fn attribute_string(&self, name: &str) -> String {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| a.args.first().cloned())
            .unwrap_or_default()
    }

    pub fn attribute_args(&self, name: &str) -> Vec<String> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.args.clone())
            .unwrap_or_default()
    }

    pub fn fields(&self) -> &[Symbol] {
        &self.fields
    }

    pub fn methods(&self) -> &[Symbol] {
        &self.methods
    }

    pub fn constructors(&self) -> &[Symbol] {
        &self.constructors
    }

    pub fn variants(&self) -> &[Symbol] {
        &self.variants
    }

    pub fn enum_members(&self) -> &[Symbol] {
        &self.enum_members
    }
}

#[derive(Debug, Clone, Default)]
pub struct SemanticModel {
    types: IndexMap<String, TypeSymbol>,
    functions: Vec<Symbol>,
}

impl SemanticModel {
    pub fn lookup_type(&self, name: &str) -> Option<&TypeSymbol> {
        self.types.get(name)
    }

    pub fn types(&self) -> impl Iterator<Item = &TypeSymbol> {
        self.types.values()
    }

    pub fn types_with(&self, attr: &str) -> Vec<&TypeSymbol> {
        self.types
            .values()
            .filter(|t| t.has_attribute(attr))
            .collect()
    }

    pub fn functions_with(&self, attr: &str) -> Vec<&Symbol> {
        self.functions
            .iter()
            .filter(|f| f.has_attribute(attr))
            .collect()
    }

    pub fn members_of(&self, type_name: &str) -> Vec<&Symbol> {
        let Some(t) = self.types.get(type_name) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        out.extend(t.fields.iter());
        out.extend(t.methods.iter());
        out.extend(t.constructors.iter());
        out.extend(t.variants.iter());
        out.extend(t.enum_members.iter());
        out
    }

    pub fn from_program(
        structs: &[StructDeclarationNode<'_>],
        enums: &[EnumDeclarationNode<'_>],
        interfaces: &[InterfaceDeclarationNode<'_>],
        functions: &[FunctionNode<'_>],
    ) -> Self {
        let mut model = SemanticModel::default();

        for s in structs {
            let kind = if s.is_ref_struct {
                TypeKind::RefStruct
            } else if s.is_value {
                TypeKind::Struct
            } else {
                TypeKind::Class
            };
            let is_shared = s.attributes.iter().any(|a| a.name.text == "shared");
            let mut fields = Vec::new();
            for f in &s.fields {
                fields.push(Symbol {
                    name: f.name.text.clone(),
                    kind: SymbolKind::Field,
                    type_name: type_display(&f.field_type),
                    visibility: f.visibility.into(),
                    declaring_type: Some(s.name.text.clone()),
                    is_static: false,
                    is_async: false,
                    is_ref: false,
                    is_variadic: false,
                    has_default: false,
                    is_weak: f.is_weak,
                    is_unowned: f.is_unowned,
                    is_unsafe: false,
                    is_shared,
                    attributes: attrs_from(&f.attributes),
                    parameters: Vec::new(),
                    payload_fields: Vec::new(),
                });
            }
            let mut methods = Vec::new();
            let mut constructors = Vec::new();
            for m in &s.methods {
                let sym = method_symbol(m, &s.name.text, is_shared);
                if m.name.text == "constructor" {
                    constructors.push(sym);
                } else {
                    methods.push(sym);
                }
            }
            let generic_param_names: Vec<String> = s
                .generic_parameters
                .as_ref()
                .map(|g| g.iter().map(|t| t.text.clone()).collect())
                .unwrap_or_default();
            let ts = TypeSymbol {
                name: s.name.text.clone(),
                display_name: s.name.text.clone(),
                kind,
                is_generic: !generic_param_names.is_empty(),
                attributes: attrs_from(&s.attributes),
                fields,
                methods,
                constructors,
                variants: Vec::new(),
                enum_members: Vec::new(),
                generic_param_names,
            };
            model.types.insert(ts.name.clone(), ts);
        }

        for e in enums {
            if e.is_data_enum() {
                let mut variants = Vec::new();
                for v in &e.variants {
                    let mut payload = Vec::new();
                    for f in &v.fields {
                        payload.push(Symbol {
                            name: f.name.text.clone(),
                            kind: SymbolKind::Field,
                            type_name: type_display(&f.field_type),
                            visibility: Visibility::Public,
                            declaring_type: Some(e.name.text.clone()),
                            is_static: false,
                            is_async: false,
                            is_ref: false,
                            is_variadic: false,
                            has_default: false,
                            is_weak: f.is_weak,
                            is_unowned: f.is_unowned,
                            is_unsafe: false,
                            is_shared: false,
                            attributes: attrs_from(&f.attributes),
                            parameters: Vec::new(),
                            payload_fields: Vec::new(),
                        });
                    }
                    variants.push(Symbol {
                        name: v.name.text.clone(),
                        kind: SymbolKind::Variant,
                        type_name: e.name.text.clone(),
                        visibility: Visibility::Public,
                        declaring_type: Some(e.name.text.clone()),
                        is_static: false,
                        is_async: false,
                        is_ref: false,
                        is_variadic: false,
                        has_default: false,
                        is_weak: false,
                        is_unowned: false,
                        is_unsafe: false,
                        is_shared: false,
                        attributes: Vec::new(),
                        parameters: Vec::new(),
                        payload_fields: payload,
                    });
                }
                let generic_param_names: Vec<String> = e
                    .generic_parameters
                    .as_ref()
                    .map(|g| g.iter().map(|t| t.text.clone()).collect())
                    .unwrap_or_default();
                let ts = TypeSymbol {
                    name: e.name.text.clone(),
                    display_name: e.name.text.clone(),
                    kind: TypeKind::Union,
                    is_generic: !generic_param_names.is_empty(),
                    attributes: attrs_from(&e.attributes),
                    fields: Vec::new(),
                    methods: Vec::new(),
                    constructors: Vec::new(),
                    variants,
                    enum_members: Vec::new(),
                    generic_param_names,
                };
                model.types.insert(ts.name.clone(), ts);
            } else {
                let mut enum_members = Vec::new();
                for v in &e.variants {
                    enum_members.push(Symbol {
                        name: v.name.text.clone(),
                        kind: SymbolKind::EnumMember,
                        type_name: "int".into(),
                        visibility: Visibility::Public,
                        declaring_type: Some(e.name.text.clone()),
                        is_static: true,
                        is_async: false,
                        is_ref: false,
                        is_variadic: false,
                        has_default: false,
                        is_weak: false,
                        is_unowned: false,
                        is_unsafe: false,
                        is_shared: false,
                        attributes: Vec::new(),
                        parameters: Vec::new(),
                        payload_fields: Vec::new(),
                    });
                }
                let ts = TypeSymbol {
                    name: e.name.text.clone(),
                    display_name: e.name.text.clone(),
                    kind: TypeKind::Enum,
                    is_generic: false,
                    attributes: attrs_from(&e.attributes),
                    fields: Vec::new(),
                    methods: Vec::new(),
                    constructors: Vec::new(),
                    variants: Vec::new(),
                    enum_members,
                    generic_param_names: Vec::new(),
                };
                model.types.insert(ts.name.clone(), ts);
            }
        }

        for i in interfaces {
            let mut methods = Vec::new();
            for m in &i.methods {
                methods.push(method_symbol(m, &i.name.text, false));
            }
            let generic_param_names: Vec<String> = i
                .generic_parameters
                .as_ref()
                .map(|g| g.iter().map(|t| t.text.clone()).collect())
                .unwrap_or_default();
            let ts = TypeSymbol {
                name: i.name.text.clone(),
                display_name: i.name.text.clone(),
                kind: TypeKind::Interface,
                is_generic: !generic_param_names.is_empty(),
                attributes: attrs_from(&i.attributes),
                fields: Vec::new(),
                methods,
                constructors: Vec::new(),
                variants: Vec::new(),
                enum_members: Vec::new(),
                generic_param_names,
            };
            model.types.insert(ts.name.clone(), ts);
        }

        for f in functions {
            model.functions.push(method_symbol(f, "", false));
        }

        model
    }
}

fn method_symbol(m: &FunctionNode<'_>, declaring: &str, is_shared: bool) -> Symbol {
    let mut parameters = Vec::new();
    for p in &m.parameters {
        parameters.push(Symbol {
            name: p.name.text.clone(),
            kind: SymbolKind::Param,
            type_name: type_display(&p.type_),
            visibility: Visibility::Public,
            declaring_type: None,
            is_static: false,
            is_async: false,
            is_ref: p.is_ref,
            is_variadic: p.is_variadic,
            has_default: p.default.is_some(),
            is_weak: false,
            is_unowned: false,
            is_unsafe: false,
            is_shared: false,
            attributes: Vec::new(),
            parameters: Vec::new(),
            payload_fields: Vec::new(),
        });
    }
    let is_unsafe = m.attributes.iter().any(|a| a.name.text == "unsafe");
    Symbol {
        name: m.name.text.clone(),
        kind: if m.name.text == "constructor" {
            SymbolKind::Constructor
        } else {
            SymbolKind::Method
        },
        type_name: m
            .return_type
            .as_ref()
            .map(type_display)
            .unwrap_or_else(|| "void".into()),
        visibility: m.visibility.into(),
        declaring_type: if declaring.is_empty() {
            None
        } else {
            Some(declaring.to_string())
        },
        is_static: m.is_static,
        is_async: m.is_async,
        is_ref: false,
        is_variadic: false,
        has_default: false,
        is_weak: false,
        is_unowned: false,
        is_unsafe,
        is_shared,
        attributes: attrs_from(&m.attributes),
        parameters,
        payload_fields: Vec::new(),
    }
}
