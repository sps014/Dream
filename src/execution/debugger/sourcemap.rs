//! Loader for the `.dbg.json` debug source map emitted by the compiler (see
//! [`crate::mir::emit::debug_map`]). Turns the on-disk JSON into lookup structures the debug adapter
//! uses to map hook ids/file ids back to source paths, function names, variable tables, and the
//! recursive **type table** that lets it decode live aggregate values from linear memory.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// A scalar (non-aggregate, non-reference) value's runtime encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Int,
    UInt,
    Byte,
    Bool,
    Char,
    Long,
    ULong,
    Float,
    Double,
}

impl ScalarKind {
    fn from_tag(tag: &str) -> ScalarKind {
        match tag {
            "uint" => ScalarKind::UInt,
            "byte" => ScalarKind::Byte,
            "bool" => ScalarKind::Bool,
            "char" => ScalarKind::Char,
            "long" => ScalarKind::Long,
            "ulong" => ScalarKind::ULong,
            "float" => ScalarKind::Float,
            "double" => ScalarKind::Double,
            _ => ScalarKind::Int,
        }
    }

    /// Byte width of the scalar in linear memory.
    pub fn width(self) -> u32 {
        match self {
            ScalarKind::Byte | ScalarKind::Bool => 1,
            ScalarKind::Long | ScalarKind::ULong | ScalarKind::Double => 8,
            _ => 4,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            ScalarKind::Int => "int",
            ScalarKind::UInt => "uint",
            ScalarKind::Byte => "byte",
            ScalarKind::Bool => "bool",
            ScalarKind::Char => "char",
            ScalarKind::Long => "long",
            ScalarKind::ULong => "ulong",
            ScalarKind::Float => "float",
            ScalarKind::Double => "double",
        }
    }
}

/// One field of a struct or a union variant payload.
#[derive(Debug, Clone)]
pub struct FieldDesc {
    pub name: String,
    pub offset: u32,
    pub type_id: u32,
}

/// One variant of a discriminated union.
#[derive(Debug, Clone)]
pub struct VariantDesc {
    pub name: String,
    pub discriminant: i32,
    pub fields: Vec<FieldDesc>,
}

/// A structural description of a runtime type: enough for the debugger to walk linear memory and
/// decode a live value. Aggregates reference their component types by index into [`SourceMap::types`].
#[derive(Debug, Clone)]
pub enum TypeDesc {
    Scalar(ScalarKind),
    Str,
    Enum,
    Struct {
        name: String,
        /// True for value (inline) structs; false for reference (heap) structs.
        value: bool,
        fields: Vec<FieldDesc>,
    },
    Union {
        name: String,
        value: bool,
        variants: Vec<VariantDesc>,
    },
    Array {
        elem: u32,
        stride: u32,
    },
    /// An opaque reference shown as an address.
    Ref,
}

#[derive(Debug, Clone)]
pub struct VarInfo {
    pub name: String,
    /// Index into the `$__dbg_v{global}` spill-pool globals.
    pub global: u32,
    /// Index into [`SourceMap::types`].
    pub type_id: u32,
}

#[derive(Debug, Clone)]
pub struct FnInfo {
    pub id: u32,
    pub name: String,
    /// Index into [`SourceMap::files`]. Retained for completeness (frames track file per-hook).
    #[allow(dead_code)]
    pub file: u32,
    pub vars: Vec<VarInfo>,
}

/// The parsed debug source map for a compiled module.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    pub files: Vec<String>,
    pub functions: Vec<FnInfo>,
    /// The recursive type table; variables and fields index into it.
    pub types: Vec<TypeDesc>,
    /// Spill-pool width; informational (the emitter sizes the globals, the debugger reads by index).
    #[allow(dead_code)]
    pub global_pool: u32,
    /// `func_id -> index into functions`.
    by_id: HashMap<u32, usize>,
}

impl SourceMap {
    /// Loads and parses the source map at `path`.
    pub fn load(path: &str) -> Result<SourceMap, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read debug map {}: {}", path, e))?;
        let v: Value =
            serde_json::from_str(&text).map_err(|e| format!("invalid debug map JSON: {}", e))?;

        let global_pool = v.get("globalPool").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        let files: Vec<String> = v
            .get("files")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let types = v
            .get("types")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(parse_type).collect())
            .unwrap_or_default();

        let mut functions = Vec::new();
        if let Some(fns) = v.get("functions").and_then(|x| x.as_array()) {
            for f in fns {
                let id = f.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let name = f
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let file = f.get("file").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let mut vars = Vec::new();
                if let Some(vs) = f.get("vars").and_then(|x| x.as_array()) {
                    for var in vs {
                        vars.push(VarInfo {
                            name: var
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            global: var.get("global").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                            type_id: var.get("type").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                        });
                    }
                }
                functions.push(FnInfo {
                    id,
                    name,
                    file,
                    vars,
                });
            }
        }

        let by_id = functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id, i))
            .collect();

        Ok(SourceMap {
            files,
            functions,
            types,
            global_pool,
            by_id,
        })
    }

    pub fn function(&self, id: u32) -> Option<&FnInfo> {
        self.by_id.get(&id).map(|i| &self.functions[*i])
    }

    pub fn file_path(&self, file_id: u32) -> Option<&str> {
        self.files.get(file_id as usize).map(String::as_str)
    }

    /// Resolves a source path from the debug client to a `file_id`, matching first on exact/canonical
    /// path and falling back to the file name so a client-supplied path that differs only in casing or
    /// symlink resolution still binds breakpoints.
    pub fn file_id_for_path(&self, path: &str) -> Option<u32> {
        let target_canon = std::fs::canonicalize(path)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        for (i, f) in self.files.iter().enumerate() {
            if f == path {
                return Some(i as u32);
            }
            if let Some(tc) = &target_canon {
                if f == tc {
                    return Some(i as u32);
                }
                if let Ok(fc) = std::fs::canonicalize(f) {
                    if fc.to_string_lossy() == *tc {
                        return Some(i as u32);
                    }
                }
            }
        }
        // Fallback: match by file name only.
        let target_name = Path::new(path).file_name();
        for (i, f) in self.files.iter().enumerate() {
            if Path::new(f).file_name() == target_name {
                return Some(i as u32);
            }
        }
        None
    }
}

fn parse_field(v: &Value) -> FieldDesc {
    FieldDesc {
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        offset: v.get("offset").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        type_id: v.get("type").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
    }
}

fn parse_type(v: &Value) -> TypeDesc {
    match v.get("kind").and_then(|x| x.as_str()).unwrap_or("ref") {
        "scalar" => TypeDesc::Scalar(ScalarKind::from_tag(
            v.get("scalar").and_then(|x| x.as_str()).unwrap_or("int"),
        )),
        "string" => TypeDesc::Str,
        "enum" => TypeDesc::Enum,
        "array" => TypeDesc::Array {
            elem: v.get("elem").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            stride: v.get("stride").and_then(|x| x.as_u64()).unwrap_or(4) as u32,
        },
        "struct" => TypeDesc::Struct {
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            value: v.get("value").and_then(|x| x.as_bool()).unwrap_or(false),
            fields: v
                .get("fields")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().map(parse_field).collect())
                .unwrap_or_default(),
        },
        "union" => TypeDesc::Union {
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            value: v.get("value").and_then(|x| x.as_bool()).unwrap_or(false),
            variants: v
                .get("variants")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .map(|vv| VariantDesc {
                            name: vv
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            discriminant: vv.get("disc").and_then(|x| x.as_i64()).unwrap_or(0)
                                as i32,
                            fields: vv
                                .get("fields")
                                .and_then(|x| x.as_array())
                                .map(|a| a.iter().map(parse_field).collect())
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        _ => TypeDesc::Ref,
    }
}
