use crate::token::syntax_token::SyntaxToken;
use std::io::Error;

/// Returns the given type name with a single trailing nullable (`?`) suffix removed.
pub fn strip_nullable(type_name: &str) -> &str {
    type_name.strip_suffix('?').unwrap_or(type_name)
}

/// Single source of truth for name mangling: joins `base` with each suffix using `_`
/// separators, e.g. base `Pair` with `["int", "string"]` becomes `Pair_int_string`.
/// With no suffixes the base name is returned unchanged.
///
/// The array suffix `[]` is rewritten to `Array` (so `char[]` -> `charArray`) because `[` and `]`
/// are not valid in a generated WASM identifier; a generic instantiated with an array type
/// argument (e.g. `Result<char[], string>`) would otherwise produce an unassemblable function
/// name. This rewrite is purely on the mangled name and is applied uniformly to type identity and
/// codegen, so the two never disagree.
pub fn mangle_with_suffixes<S: AsRef<str>>(
    base: &str,
    suffixes: impl IntoIterator<Item = S>,
) -> String {
    let mut name = base.to_string();
    for suffix in suffixes {
        name.push('_');
        name.push_str(&suffix.as_ref().replace("[]", "Array"));
    }
    name
}

/// Builds the monomorphized name for a generic instantiation by appending every concrete
/// type argument, e.g. base `Pair` with `[int, string]` becomes `Pair_int_string`. With no
/// arguments the base name is returned unchanged.
pub fn mangle_generic(base: &str, args: &[Type]) -> String {
    mangle_with_suffixes(base, args.iter().map(|arg| arg.get_type()))
}

/// Maps a C#/.NET-style type name to its canonical Dream primitive spelling, or returns
/// `None` if `name` is not a recognized alias. `String`/`Int32`/... become
/// `string`/`int`/..., so the two spellings are fully interchangeable while every
/// downstream stage continues to see the lowercase canonical names.
pub fn canonical_type_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "String" => "string",
        "Int32" => "int",
        "Int64" => "long",
        "UInt32" => "uint",
        "UInt64" => "ulong",
        "Byte" => "byte",
        "Single" => "float",
        "Double" => "double",
        "Boolean" => "bool",
        "Char" => "char",
        "Object" => "object",
        "Void" => "void",
        _ => return None,
    })
}

/// Constructs the primitive `Type` named by `name`, backed by `token`, or returns `None`
/// if `name` does not denote a primitive. Single source of truth for primitive construction.
pub fn primitive_type(name: &str, token: SyntaxToken) -> Option<Type> {
    Some(match name {
        "int" => Type::Integer(token),
        "float" => Type::Float(token),
        "double" => Type::Double(token),
        "string" => Type::String(token),
        "bool" => Type::Boolean(token),
        "char" => Type::Char(token),
        "long" => Type::Long(token),
        "uint" => Type::UInt(token),
        "ulong" => Type::ULong(token),
        "byte" => Type::Byte(token),
        _ => return None,
    })
}

/// The scalar primitive type names that own runtime metadata (boxing, array helpers, tags).
/// `string` is included here because it is a first-class value type even though it is a heap
/// reference; it is excluded from [`is_boxable_primitive`]. Single source of truth for the
/// repeated `"int" | "float" | ...` lists that were previously copied across codegen modules.
pub const PRIMITIVE_TYPE_NAMES: [&str; 10] = [
    "int", "float", "double", "bool", "char", "string", "long", "uint", "ulong", "byte",
];

/// True for the scalar primitives that are boxed into a small tagged heap block when widened to
/// `object` (everything except `string`, which is already a heap reference).
pub fn is_boxable_primitive(name: &str) -> bool {
    matches!(
        name,
        "int" | "float" | "double" | "bool" | "char" | "long" | "uint" | "ulong" | "byte"
    )
}

/// True for the numeric primitives that participate in implicit widening. The single predicate
/// behind overload viability and assignment/cast compatibility.
pub fn is_numeric_primitive(name: &str) -> bool {
    matches!(
        name,
        "int" | "float" | "double" | "long" | "uint" | "ulong" | "byte"
    )
}

/// Returns the given type name with a single trailing array (`[]`) suffix removed.
pub fn strip_array(type_name: &str) -> &str {
    type_name.strip_suffix("[]").unwrap_or(type_name)
}

/// The reserved member name of a type's constructor declaration (`constructor(...) { ... }`).
/// The parser recognizes it, semantics validates it, and codegen mangles it (via the backend
/// `constructor_fn`). Single source of truth so the spelling never drifts between layers.
pub const CONSTRUCTOR_NAME: &str = "constructor";

/// The reserved member name of a type's destructor declaration (`del() { ... }`), run on
/// scope exit. Single source of truth shared by parser/semantics/codegen.
pub const DESTRUCTOR_NAME: &str = "del";

/// True if `name` is a reserved member declaration keyword (`constructor`/`del`) that the parser
/// accepts without the leading `fun` and that cannot be marked `public` or carry a return type.
pub fn is_special_member_name(name: &str) -> bool {
    name == CONSTRUCTOR_NAME || name == DESTRUCTOR_NAME
}

/// The synthetic loop-index local the parser emits when lowering a `for-each`; `n` is a
/// per-loop counter that keeps nested loops from colliding. Centralized so the parser is the
/// only place that knows the spelling.
pub fn foreach_index_local(n: usize) -> String {
    format!("__foreach_idx_{}", n)
}

/// The synthetic array-holder local the parser emits when lowering a `for-each`. See
/// [`foreach_index_local`].
pub fn foreach_array_local(n: usize) -> String {
    format!("__foreach_arr_{}", n)
}

/// The base type name of the async handle `Future<T>`. Single source of truth for the identifier
/// the async machinery keys on (the structured `Future<T>` type and its `Future_<inner>` mangling).
pub const FUTURE_TYPE: &str = "Future";



/// Represents a data type in the language
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Integer(SyntaxToken),
    Float(SyntaxToken),
    Double(SyntaxToken),
    String(SyntaxToken),
    Boolean(SyntaxToken),
    /// A single character. Stored as an `i32` code point on the stack but only one byte in
    /// memory (arrays/fields use `i32.load8_u`/`i32.store8`). A value type (not ref-counted).
    Char(SyntaxToken),
    /// A signed 64-bit integer. Represented as an `i64` on the stack and 8 bytes in memory.
    Long(SyntaxToken),
    /// An unsigned 32-bit integer. Represented as an `i32` on the stack (4 bytes in memory) but
    /// uses unsigned WASM ops (`div_u`/`lt_u`/...).
    UInt(SyntaxToken),
    /// An unsigned 64-bit integer. Represented as an `i64` on the stack (8 bytes in memory) and
    /// uses unsigned WASM ops.
    ULong(SyntaxToken),
    /// An unsigned 8-bit integer. Stored as an `i32` on the stack but only one byte in memory
    /// (`i32.load8_u`/`i32.store8`, like `char`). A value type (not ref-counted).
    Byte(SyntaxToken),
    /// The universal top type. At runtime an `object` is an `i32` pointer to a tagged heap
    /// block: primitives are boxed, reference types are stored directly (their block carries
    /// the tag in its header).
    Object(SyntaxToken),
    Array(Box<Type>),
    Struct(SyntaxToken, Option<Vec<Type>>),
    Generic(String),
    Nullable(Box<Type>),
    /// A first-class function value `fun(params...): ret`. Represented at runtime as an `i32`
    /// index into the module's function table (used with `call_indirect`).
    Function(Vec<Type>, Box<Type>),
    Void,
    /// The "poison" type produced on a semantic error (e.g. an unresolved identifier or call).
    /// It is assignable to and from every type so a single mistake does not cascade into a flood
    /// of follow-on diagnostics. It exists only during analysis of erroneous programs; codegen
    /// never runs once any error is reported, so it never has to be lowered.
    Unknown,
}

/// The sentinel type name carried by [`Type::Unknown`]. Chosen with angle brackets so it can never
/// collide with a user-declared identifier.
pub const UNKNOWN_TYPE_NAME: &str = "<unknown>";

/// True if `name` is the poison sentinel produced on type errors (see [`Type::Unknown`]).
pub fn is_unknown_type_name(name: &str) -> bool {
    // Strip any nullable/array suffixes a caller may have appended before comparing.
    let base = strip_array(strip_nullable(name));
    base == UNKNOWN_TYPE_NAME
}

impl Type {
    /// Returns the string representation of the type
    pub fn get_type(&self) -> String {
        match self {
            Type::Integer(_) => "int".to_string(),
            Type::Float(_) => "float".to_string(),
            Type::Double(_) => "double".to_string(),
            Type::String(_) => "string".to_string(),
            Type::Object(_) => "object".to_string(),
            Type::Void => "void".to_string(),
            Type::Boolean(_) => "bool".to_string(),
            Type::Char(_) => "char".to_string(),
            Type::Long(_) => "long".to_string(),
            Type::UInt(_) => "uint".to_string(),
            Type::ULong(_) => "ulong".to_string(),
            Type::Byte(_) => "byte".to_string(),
            Type::Array(inner) => format!("{}[]", inner.get_type()),
            Type::Struct(token, generic_args) => match generic_args {
                Some(args) => mangle_generic(&token.text, args),
                None => token.text.clone(),
            },
            Type::Generic(name) => name.clone(),
            Type::Nullable(inner) => format!("{}?", inner.get_type()),
            Type::Function(params, ret) => {
                let params_str = params
                    .iter()
                    .map(|p| p.get_type())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("fun({}):{}", params_str, ret.get_type())
            }
            Type::Unknown => UNKNOWN_TYPE_NAME.to_string(),
        }
    }

    /// Human-readable spelling of the type, the inverse of how it is written in source. Unlike
    /// [`get_type`](Self::get_type), generic instantiations render with angle brackets
    /// (`Box<int>`, `Pair<int, string>`, `Box<Box<int>>`) rather than the `_`-mangled monomorphized
    /// name (`Box_int`). Use this anywhere a type is shown to a human (hovers, inlay hints,
    /// signatures); use `get_type` for internal identity/mangling.
    pub fn display_name(&self) -> String {
        match self {
            Type::Array(inner) => format!("{}[]", inner.display_name()),
            Type::Struct(token, generic_args) => match generic_args {
                Some(args) => {
                    let args_str = args
                        .iter()
                        .map(|a| a.display_name())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}<{}>", token.text, args_str)
                }
                None => token.text.clone(),
            },
            Type::Nullable(inner) => format!("{}?", inner.display_name()),
            Type::Function(params, ret) => {
                let params_str = params
                    .iter()
                    .map(|p| p.display_name())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fun({}): {}", params_str, ret.display_name())
            }
            Type::Unknown => "unknown".to_string(),
            // Primitives and bare generic parameters spell the same either way.
            _ => self.get_type(),
        }
    }

    /// True if this is the poison [`Type::Unknown`] produced on a semantic error.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Type::Unknown)
    }

    /// Returns true if this type is a nullable (`T?`) type.
    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Nullable(_))
    }

    /// True if this is the primitive `bool` type (a bare `bool`, not `bool?`/`bool[]`). Structural
    /// replacement for `get_type() == "bool"`.
    pub fn is_bool(&self) -> bool {
        matches!(self, Type::Boolean(_))
    }

    /// True if this is the primitive `int` type (a bare `int`). Structural replacement for
    /// `get_type() == "int"`.
    pub fn is_int(&self) -> bool {
        matches!(self, Type::Integer(_))
    }

    /// True if this is the primitive `string` type. Structural replacement for
    /// `get_type() == "string"`.
    pub fn is_string(&self) -> bool {
        matches!(self, Type::String(_))
    }

    /// True if this is the `object` top type. Structural replacement for `get_type() == "object"`.
    pub fn is_object(&self) -> bool {
        matches!(self, Type::Object(_))
    }

    /// Returns true if this type is an array (`T[]`) type.
    pub fn is_array(&self) -> bool {
        matches!(self, Type::Array(_))
    }

    /// Returns the type name with any trailing nullable (`?`) suffix removed.
    pub fn base_name(&self) -> String {
        strip_nullable(&self.get_type()).to_string()
    }

    /// Returns the source span of the token backing this type, if any.
    /// Composite types (arrays, nullables) defer to their inner type; `Void`/`Generic`
    /// have no backing token and return `None`.
    pub fn get_span(&self) -> Option<dream_text::text_span::TextSpan> {
        match self {
            Type::Integer(token)
            | Type::Float(token)
            | Type::Double(token)
            | Type::String(token)
            | Type::Boolean(token)
            | Type::Char(token)
            | Type::Long(token)
            | Type::UInt(token)
            | Type::ULong(token)
            | Type::Byte(token)
            | Type::Object(token)
            | Type::Struct(token, _) => Some(token.position),
            Type::Array(inner) | Type::Nullable(inner) => inner.get_span(),
            Type::Void | Type::Generic(_) | Type::Function(_, _) | Type::Unknown => None,
        }
    }

    /// Returns the line and column string of the type token
    pub fn get_line_str(&self) -> String {
        match self {
            Type::Integer(token) => token.position.get_point_str(),
            Type::Float(token) => token.position.get_point_str(),
            Type::Double(token) => token.position.get_point_str(),
            Type::String(token) => token.position.get_point_str(),
            Type::Object(token) => token.position.get_point_str(),
            Type::Void => "".to_string(),
            Type::Boolean(token) => token.position.get_point_str(),
            Type::Char(token) => token.position.get_point_str(),
            Type::Long(token) => token.position.get_point_str(),
            Type::UInt(token) => token.position.get_point_str(),
            Type::ULong(token) => token.position.get_point_str(),
            Type::Byte(token) => token.position.get_point_str(),
            Type::Array(inner) => inner.get_line_str(),
            Type::Struct(token, _) => token.position.get_point_str(),
            Type::Generic(_) => "".to_string(), // Can be improved
            Type::Nullable(inner) => inner.get_line_str(),
            Type::Function(_, _) => "".to_string(),
            Type::Unknown => "".to_string(),
        }
    }

    /// Parses a Type from a given SyntaxToken
    pub fn from_token(mut token: SyntaxToken) -> Result<Type, Error> {
        // Normalize C#/.NET-style type names (String, Int32, ...) to their canonical Dream
        // primitive spelling before any further classification, so the two are interchangeable.
        if let Some(canonical) = canonical_type_name(&token.text) {
            token.text = canonical.to_string();
        }
        if let Some(primitive) = primitive_type(&token.text, token.clone()) {
            return Ok(primitive);
        }
        let r = match token.text.as_str() {
            "object" => Type::Object(token),
            "void" => Type::Void,
            _ => {
                if token.text.ends_with("?") {
                    let base_type_str = &token.text[0..token.text.len() - 1];
                    let mut base_token = token.clone();
                    base_token.text = base_type_str.to_string();
                    let base_type = Type::from_token(base_token)?;

                    // Restrict nullable to reference types
                    match &base_type {
                        Type::String(_)
                        | Type::Object(_)
                        | Type::Array(_)
                        | Type::Struct(_, _)
                        | Type::Void => {
                            return Ok(Type::Nullable(Box::new(base_type)));
                        }
                        _ => {
                            return Err(Error::other(format!("Type '{}' cannot be nullable. Only reference types (string, arrays, structs) can be nullable.", base_type.get_type())));
                        }
                    }
                }
                // Handle array types like "int[]" or "Point[]"
                if token.text.ends_with("[]") {
                    let base_type_str = &token.text[0..token.text.len() - 2];
                    let mut base_token = token.clone();
                    base_token.text = base_type_str.to_string();
                    let base_type = Type::from_token(base_token)?;
                    return Ok(Type::Array(Box::new(base_type)));
                }
                // If it's not a built-in type or array, assume it's a struct type
                return Ok(Type::Struct(token, None));
            }
        };
        Ok(r)
    }
}
