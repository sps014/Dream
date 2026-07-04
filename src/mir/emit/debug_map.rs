//! Debug-info source map: metadata connecting the emitted WASM's debug hooks back to Dream source.
//!
//! When the compiler runs with debug-info enabled, the backend instruments each function with
//! `dream_debug.enter`/`line`/`exit` host-hook calls and spills every named local into a pool of
//! exported `i64` globals (`$__dbg_v{k}`) at each statement boundary. This module builds the
//! metadata the [debugger](crate::execution) needs to make sense of those hooks — the file table,
//! per-function variable tables, and the global-pool width — and serializes it to a compact JSON
//! sidecar (`<stem>.dbg.json`) that ships next to the `.wat`/`.wasm`.

use crate::mir::{Mir, MirFunction, Statement};
use crate::types::{PrimTy, TyKind, TypeId, TypeInterner};

/// The name of the host module the debug hooks are imported from.
pub const DEBUG_MODULE: &str = "dream_debug";

/// How the host should reinterpret a variable's spilled 64-bit slot. Every named local is spilled
/// into an `i64` global (zero-extended / bit-reinterpreted as needed); the kind tells the debugger
/// how to decode those bits back into a displayable value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugVarKind {
    Int,
    UInt,
    Byte,
    Bool,
    Char,
    Long,
    ULong,
    Float,
    Double,
    /// A `string` pointer: the low 32 bits address a heap string block the host reads.
    Str,
    /// Any other reference/pointer (objects, arrays, structs): shown as an address.
    Ref,
}

impl DebugVarKind {
    /// The stable string tag serialized into the source map and matched by the debugger.
    pub fn tag(self) -> &'static str {
        match self {
            DebugVarKind::Int => "int",
            DebugVarKind::UInt => "uint",
            DebugVarKind::Byte => "byte",
            DebugVarKind::Bool => "bool",
            DebugVarKind::Char => "char",
            DebugVarKind::Long => "long",
            DebugVarKind::ULong => "ulong",
            DebugVarKind::Float => "float",
            DebugVarKind::Double => "double",
            DebugVarKind::Str => "string",
            DebugVarKind::Ref => "ref",
        }
    }
}

/// Classifies a local's interned type into the host-facing [`DebugVarKind`].
pub fn classify(interner: &TypeInterner, ty: TypeId) -> DebugVarKind {
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Int) => DebugVarKind::Int,
        TyKind::Prim(PrimTy::UInt) => DebugVarKind::UInt,
        TyKind::Prim(PrimTy::Byte) => DebugVarKind::Byte,
        TyKind::Prim(PrimTy::Bool) => DebugVarKind::Bool,
        TyKind::Prim(PrimTy::Char) => DebugVarKind::Char,
        TyKind::Prim(PrimTy::Long) => DebugVarKind::Long,
        TyKind::Prim(PrimTy::ULong) => DebugVarKind::ULong,
        TyKind::Prim(PrimTy::Float) => DebugVarKind::Float,
        TyKind::Prim(PrimTy::Double) => DebugVarKind::Double,
        TyKind::Prim(PrimTy::String) => DebugVarKind::Str,
        // C-style enums are `i32` discriminants at runtime.
        TyKind::Enum(_) => DebugVarKind::Int,
        _ => DebugVarKind::Ref,
    }
}

/// One named local surfaced to the debugger.
#[derive(Debug, Clone)]
pub struct DebugVar {
    /// Source name of the local (as written by the user).
    pub name: String,
    /// The WASM local index (`$local`) the value is read from when spilling.
    pub local: u32,
    /// The `$__dbg_v{global}` pool slot this variable is spilled into at each statement boundary.
    pub global: u32,
    pub kind: DebugVarKind,
}

/// Per-function debug metadata: its stable id (matching the `enter`/`exit` hook argument), its
/// symbol/display name, source file, and variable table.
#[derive(Debug, Clone)]
pub struct DebugFunction {
    pub id: u32,
    /// The emitted `$symbol` (without the `$`) — matches the WAT function name.
    pub symbol: String,
    /// The user-facing function name.
    pub name: String,
    /// Index into [`DebugModule::files`].
    pub file: u32,
    pub vars: Vec<DebugVar>,
}

/// Whole-module debug metadata assembled during codegen and serialized to the `.dbg.json` sidecar.
#[derive(Debug, Clone, Default)]
pub struct DebugModule {
    /// `file_id -> absolute source path`. Referenced by `dream_debug.line(file_id, line)` hooks.
    pub files: Vec<String>,
    pub functions: Vec<DebugFunction>,
    /// Number of `i64` globals in the spill pool (the max named-local count across all functions).
    pub global_pool: u32,
}

impl DebugModule {
    /// Builds the module-wide debug metadata for `mir`: a stable file table, a variable table for
    /// every non-async function that carries a source file, and the global-pool width. `symbols`
    /// maps `(def, instance)` to the emitted symbol so ids line up with the WAT.
    pub fn build(
        mir: &Mir,
        interner: &TypeInterner,
        symbols: &std::collections::HashMap<(crate::types::DefId, Vec<TypeId>), String>,
    ) -> DebugModule {
        let mut files: Vec<String> = Vec::new();
        let file_id = |path: &str, files: &mut Vec<String>| -> u32 {
            if let Some(i) = files.iter().position(|f| f == path) {
                i as u32
            } else {
                files.push(path.to_string());
                (files.len() - 1) as u32
            }
        };

        let mut functions = Vec::new();
        let mut global_pool = 0u32;
        for f in &mir.functions {
            // Async functions are not line-instrumented in v1; skip them (and any function with no
            // source file, e.g. the synthesized module-init).
            if f.is_async || f.file.is_none() {
                continue;
            }
            // Only functions that actually carry a line marker are debuggable; skip the rest so
            // runtime helpers merged without markers do not clutter the map.
            if !function_has_debug_line(f) {
                continue;
            }
            let path = f.file.as_deref().unwrap();
            let fid = file_id(path, &mut files);
            let vars = debug_vars(f, interner);
            global_pool = global_pool.max(vars.len() as u32);
            let symbol = symbols
                .get(&(f.def, f.instance.clone()))
                .cloned()
                .unwrap_or_else(|| f.name.clone());
            functions.push(DebugFunction {
                id: functions.len() as u32,
                symbol,
                name: f.name.clone(),
                file: fid,
                vars,
            });
        }

        DebugModule {
            files,
            functions,
            global_pool,
        }
    }

    /// Serializes to the compact JSON consumed by the debugger. Hand-written (no serde dependency)
    /// so it works in every build configuration of the compiler crate.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\n");
        s.push_str("  \"version\": 1,\n");
        s.push_str(&format!("  \"globalPool\": {},\n", self.global_pool));
        s.push_str("  \"files\": [");
        for (i, f) in self.files.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("\"{}\"", json_escape(f)));
        }
        s.push_str("],\n");
        s.push_str("  \"functions\": [\n");
        for (i, func) in self.functions.iter().enumerate() {
            if i > 0 {
                s.push_str(",\n");
            }
            s.push_str(&format!(
                "    {{\"id\": {}, \"symbol\": \"{}\", \"name\": \"{}\", \"file\": {}, \"vars\": [",
                func.id,
                json_escape(&func.symbol),
                json_escape(&func.name),
                func.file
            ));
            for (j, v) in func.vars.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                s.push_str(&format!(
                    "{{\"name\": \"{}\", \"global\": {}, \"kind\": \"{}\"}}",
                    json_escape(&v.name),
                    v.global,
                    v.kind.tag()
                ));
            }
            s.push_str("]}");
        }
        s.push_str("\n  ]\n}\n");
        s
    }
}

/// True if `func` contains at least one `DebugLine` marker (i.e. was compiled with debug-info).
fn function_has_debug_line(func: &MirFunction) -> bool {
    func.blocks
        .iter()
        .any(|b| b.stmts.iter().any(|s| matches!(s, Statement::DebugLine(_))))
}

/// Builds the ordered variable table for a function: every named local (parameters first, then
/// declared `let`s), each assigned a spill-pool slot equal to its position in the table.
pub fn debug_vars(func: &MirFunction, interner: &TypeInterner) -> Vec<DebugVar> {
    let mut vars = Vec::new();
    for (i, decl) in func.locals.iter().enumerate() {
        let Some(name) = decl.name.as_ref() else {
            continue;
        };
        vars.push(DebugVar {
            name: name.clone(),
            local: i as u32,
            global: vars.len() as u32,
            kind: classify(interner, decl.ty),
        });
    }
    vars
}

/// Escapes a string for embedding in the hand-written JSON (quotes, backslashes, control chars).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
