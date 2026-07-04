//! Loader for the `.dbg.json` debug source map emitted by the compiler (see
//! [`crate::mir::emit::debug_map`]). Turns the on-disk JSON into lookup structures the debug adapter
//! uses to map hook ids/file ids back to source paths, function names, and variable tables.

use std::collections::HashMap;
use std::path::Path;

/// How a spilled `i64` variable slot should be decoded for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Int,
    UInt,
    Byte,
    Bool,
    Char,
    Long,
    ULong,
    Float,
    Double,
    Str,
    Ref,
}

impl VarKind {
    fn from_tag(tag: &str) -> VarKind {
        match tag {
            "int" => VarKind::Int,
            "uint" => VarKind::UInt,
            "byte" => VarKind::Byte,
            "bool" => VarKind::Bool,
            "char" => VarKind::Char,
            "long" => VarKind::Long,
            "ulong" => VarKind::ULong,
            "float" => VarKind::Float,
            "double" => VarKind::Double,
            "string" => VarKind::Str,
            _ => VarKind::Ref,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VarInfo {
    pub name: String,
    /// Index into the `$__dbg_v{global}` spill-pool globals.
    pub global: u32,
    pub kind: VarKind,
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
        let v: serde_json::Value =
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
                            kind: VarKind::from_tag(
                                var.get("kind").and_then(|x| x.as_str()).unwrap_or("ref"),
                            ),
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
