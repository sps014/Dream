//! Debug-info source map: metadata connecting the emitted WASM's debug hooks back to Dream source.
//!
//! When the compiler runs with debug-info enabled, the backend instruments each function with
//! `dream_debug.enter`/`line`/`exit` host-hook calls and spills every named local into a pool of
//! exported `i64` globals (`$__dbg_v{k}`) at each statement boundary. This module builds the
//! metadata the [debugger](crate::execution) needs to make sense of those hooks — the file table,
//! per-function variable tables, and a full **type table** describing struct fields / union variants
//! / array elements so the debugger can recursively decode live aggregate values (not just show a
//! raw pointer) — and serializes it to a compact JSON sidecar (`<stem>.dbg.json`) shipped next to
//! the `.wat`/`.wasm`.

use crate::hir::{scalar_size, LayoutTable};
use crate::mir::{Mir, MirFunction, Statement};
use crate::types::{PrimTy, TyKind, TypeId, TypeInterner};
use std::collections::HashMap;

/// The name of the host module the debug hooks are imported from.
pub const DEBUG_MODULE: &str = "dream_debug";

/// How a named local is stored in its WASM local slot, so the emitter can widen/reinterpret it into
/// the `i64` spill pool losslessly. (Aggregates and references are `i32` pointers.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpillKind {
    I32,
    I64,
    F32,
    F64,
}

/// Computes the spill kind for a local's type — the WASM storage class of its slot.
pub fn spill_kind(interner: &TypeInterner, ty: TypeId) -> SpillKind {
    // Value structs/unions are held in a local as an `i32` address (pointer into the shadow stack).
    if interner.value_layout(ty).is_some() {
        return SpillKind::I32;
    }
    match interner.kind(interner.strip_nullable(ty)) {
        TyKind::Prim(PrimTy::Double) => SpillKind::F64,
        TyKind::Prim(PrimTy::Float) => SpillKind::F32,
        TyKind::Prim(PrimTy::Long | PrimTy::ULong) => SpillKind::I64,
        _ => SpillKind::I32,
    }
}

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
    fn tag(self) -> &'static str {
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

/// One field of a struct or a union variant.
#[derive(Debug, Clone)]
pub struct FieldDesc {
    pub name: String,
    pub offset: u32,
    /// Index into [`DebugModule::types`].
    pub type_id: u32,
}

/// One variant of a discriminated union.
#[derive(Debug, Clone)]
pub struct VariantDesc {
    pub name: String,
    pub discriminant: i32,
    pub fields: Vec<FieldDesc>,
}

/// A structural description of a runtime type, sufficient for the debugger to walk memory and decode
/// live values. Recursive: aggregates reference their component types by index into
/// [`DebugModule::types`].
#[derive(Debug, Clone)]
pub enum TypeDesc {
    Scalar(ScalarKind),
    /// A `string`: pointer to `[len:i32][utf8...]`.
    Str,
    /// C-style enum (an `i32` discriminant).
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
        /// Byte stride between consecutive elements.
        stride: u32,
    },
    /// An opaque reference (interface/object/function value, or a type with no known layout): shown
    /// as an address.
    Ref,
}

/// Builds and memoizes the module's [`TypeDesc`] registry, breaking recursive types with a
/// placeholder slot reserved before descending into components.
struct TypeRegistry<'a> {
    interner: &'a TypeInterner,
    layouts: &'a LayoutTable,
    descs: Vec<TypeDesc>,
    by_ty: HashMap<TypeId, u32>,
}

impl<'a> TypeRegistry<'a> {
    fn new(interner: &'a TypeInterner, layouts: &'a LayoutTable) -> Self {
        TypeRegistry {
            interner,
            layouts,
            descs: Vec::new(),
            by_ty: HashMap::new(),
        }
    }

    /// Interns `ty`, returning its index into `descs` (registering it and its components first).
    fn intern(&mut self, ty: TypeId) -> u32 {
        let key = self.interner.strip_nullable(ty);
        if let Some(&idx) = self.by_ty.get(&key) {
            return idx;
        }
        // Reserve the slot up front (placeholder) so a self-referential type terminates.
        let idx = self.descs.len() as u32;
        self.by_ty.insert(key, idx);
        self.descs.push(TypeDesc::Ref);
        let desc = self.build(key);
        self.descs[idx as usize] = desc;
        idx
    }

    fn build(&mut self, ty: TypeId) -> TypeDesc {
        // Structs (value or reference) — keyed by the monomorphized type id.
        if let Some(layout) = self.layouts.get(ty).cloned() {
            let value = self.interner.is_value_type(ty);
            let fields = layout
                .fields
                .iter()
                .map(|f| FieldDesc {
                    name: f.name.clone(),
                    offset: f.offset,
                    type_id: self.intern(f.ty),
                })
                .collect();
            return TypeDesc::Struct {
                name: layout.name,
                value,
                fields,
            };
        }
        if let Some(u) = self.layouts.union(ty).cloned() {
            let value = self.interner.is_value_type(ty);
            let variants = u
                .variants
                .iter()
                .map(|v| VariantDesc {
                    name: v.name.clone(),
                    discriminant: v.discriminant,
                    fields: v
                        .fields
                        .iter()
                        .map(|f| FieldDesc {
                            name: f.name.clone(),
                            offset: f.offset,
                            type_id: self.intern(f.ty),
                        })
                        .collect(),
                })
                .collect();
            return TypeDesc::Union {
                name: u.name,
                value,
                variants,
            };
        }
        match self.interner.kind(ty) {
            TyKind::Prim(PrimTy::Int) => TypeDesc::Scalar(ScalarKind::Int),
            TyKind::Prim(PrimTy::UInt) => TypeDesc::Scalar(ScalarKind::UInt),
            TyKind::Prim(PrimTy::Byte) => TypeDesc::Scalar(ScalarKind::Byte),
            TyKind::Prim(PrimTy::Bool) => TypeDesc::Scalar(ScalarKind::Bool),
            TyKind::Prim(PrimTy::Char) => TypeDesc::Scalar(ScalarKind::Char),
            TyKind::Prim(PrimTy::Long) => TypeDesc::Scalar(ScalarKind::Long),
            TyKind::Prim(PrimTy::ULong) => TypeDesc::Scalar(ScalarKind::ULong),
            TyKind::Prim(PrimTy::Float) => TypeDesc::Scalar(ScalarKind::Float),
            TyKind::Prim(PrimTy::Double) => TypeDesc::Scalar(ScalarKind::Double),
            TyKind::Prim(PrimTy::String) => TypeDesc::Str,
            TyKind::Enum(_) => TypeDesc::Enum,
            TyKind::Array(elem) => {
                let elem = *elem;
                let stride = scalar_size(self.interner, elem).0;
                let elem_id = self.intern(elem);
                TypeDesc::Array {
                    elem: elem_id,
                    stride,
                }
            }
            _ => TypeDesc::Ref,
        }
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
    /// How the local is spilled (its WASM storage class).
    pub spill: SpillKind,
    /// Index into [`DebugModule::types`] describing the variable's structure.
    pub type_id: u32,
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
    /// The recursive type table; variables and fields index into it.
    pub types: Vec<TypeDesc>,
    /// Number of `i64` globals in the spill pool (the max named-local count across all functions).
    pub global_pool: u32,
}

impl DebugModule {
    /// Builds the module-wide debug metadata for `mir`: a stable file table, a variable table for
    /// every non-async function that carries a source file, the shared type table, and the
    /// global-pool width. `symbols` maps `(def, instance)` to the emitted symbol so ids line up with
    /// the WAT.
    pub fn build(
        mir: &Mir,
        interner: &TypeInterner,
        symbols: &HashMap<(crate::types::DefId, Vec<TypeId>), String>,
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

        // Standard-library/prelude source files (e.g. `<std>/core/string.dream`); functions from
        // these are hidden from the debugger so the call stack shows only user frames and stepping
        // never descends into stdlib. Async functions ARE instrumented (their poll body carries the
        // same line markers), so `is_async` is deliberately not a skip condition here.
        let prelude_files: std::collections::HashSet<&str> = crate::stdlib::PRELUDE_FILES
            .iter()
            .map(|(n, _)| *n)
            .collect();

        let mut registry = TypeRegistry::new(interner, &mir.layouts);
        let mut functions = Vec::new();
        let mut global_pool = 0u32;
        for f in &mir.functions {
            // Skip functions with no source file (e.g. the synthesized module-init and generated
            // glue) and any function defined in a stdlib/prelude file.
            match f.file.as_deref() {
                None => continue,
                Some(path) if prelude_files.contains(path) => continue,
                _ => {}
            }
            // For async functions the MIR `f` is only a stub: the real body (and, crucially, the
            // local numbering the poll uses) is rebuilt from `hir_fn`. Lower it exactly like the
            // emitter does so `DebugVar::local` indices line up with the poll's WASM locals.
            let lowered;
            let body: &MirFunction = if f.is_async {
                match f.hir_fn.as_ref() {
                    Some(hir) => {
                        lowered = crate::mir::lower::lower_async_poll_body(hir, interner);
                        &lowered
                    }
                    None => continue,
                }
            } else {
                f
            };
            // Only functions that actually carry a line marker are debuggable; skip the rest so
            // runtime helpers merged without markers do not clutter the map.
            if !function_has_debug_line(body) {
                continue;
            }
            let path = f.file.as_deref().unwrap();
            let fid = file_id(path, &mut files);
            let vars = debug_vars(body, interner, &mut registry);
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
            types: registry.descs,
            global_pool,
        }
    }

    /// Serializes to the compact JSON consumed by the debugger. Hand-written (no serde dependency)
    /// so it works in every build configuration of the compiler crate.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\n");
        s.push_str("  \"version\": 2,\n");
        s.push_str(&format!("  \"globalPool\": {},\n", self.global_pool));
        s.push_str("  \"files\": [");
        for (i, f) in self.files.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("\"{}\"", json_escape(f)));
        }
        s.push_str("],\n");

        // Type table.
        s.push_str("  \"types\": [\n");
        for (i, t) in self.types.iter().enumerate() {
            if i > 0 {
                s.push_str(",\n");
            }
            s.push_str("    ");
            s.push_str(&type_to_json(t));
        }
        s.push_str("\n  ],\n");

        // Functions + variable tables.
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
                    "{{\"name\": \"{}\", \"global\": {}, \"type\": {}}}",
                    json_escape(&v.name),
                    v.global,
                    v.type_id
                ));
            }
            s.push_str("]}");
        }
        s.push_str("\n  ]\n}\n");
        s
    }
}

fn field_to_json(f: &FieldDesc) -> String {
    format!(
        "{{\"name\": \"{}\", \"offset\": {}, \"type\": {}}}",
        json_escape(&f.name),
        f.offset,
        f.type_id
    )
}

fn type_to_json(t: &TypeDesc) -> String {
    match t {
        TypeDesc::Scalar(k) => format!("{{\"kind\": \"scalar\", \"scalar\": \"{}\"}}", k.tag()),
        TypeDesc::Str => "{\"kind\": \"string\"}".to_string(),
        TypeDesc::Enum => "{\"kind\": \"enum\"}".to_string(),
        TypeDesc::Ref => "{\"kind\": \"ref\"}".to_string(),
        TypeDesc::Array { elem, stride } => {
            format!(
                "{{\"kind\": \"array\", \"elem\": {}, \"stride\": {}}}",
                elem, stride
            )
        }
        TypeDesc::Struct {
            name,
            value,
            fields,
        } => {
            let fs: Vec<String> = fields.iter().map(field_to_json).collect();
            format!(
                "{{\"kind\": \"struct\", \"name\": \"{}\", \"value\": {}, \"fields\": [{}]}}",
                json_escape(name),
                value,
                fs.join(",")
            )
        }
        TypeDesc::Union {
            name,
            value,
            variants,
        } => {
            let vs: Vec<String> = variants
                .iter()
                .map(|v| {
                    let fs: Vec<String> = v.fields.iter().map(field_to_json).collect();
                    format!(
                        "{{\"name\": \"{}\", \"disc\": {}, \"fields\": [{}]}}",
                        json_escape(&v.name),
                        v.discriminant,
                        fs.join(",")
                    )
                })
                .collect();
            format!(
                "{{\"kind\": \"union\", \"name\": \"{}\", \"value\": {}, \"variants\": [{}]}}",
                json_escape(name),
                value,
                vs.join(",")
            )
        }
    }
}

/// True if `func` contains at least one `DebugLine` marker (i.e. was compiled with debug-info).
fn function_has_debug_line(func: &MirFunction) -> bool {
    func.blocks
        .iter()
        .any(|b| b.stmts.iter().any(|s| matches!(s, Statement::DebugLine(_))))
}

/// Builds the ordered variable table for a function: every named user local (parameters first, then
/// declared `let`s), each assigned a spill-pool slot equal to its position in the table. Synthetic
/// compiler temporaries (names beginning with `__`) are omitted so they neither spill nor appear in
/// the debugger's variables view.
fn debug_vars(
    func: &MirFunction,
    interner: &TypeInterner,
    registry: &mut TypeRegistry,
) -> Vec<DebugVar> {
    let mut vars = Vec::new();
    for (i, decl) in func.locals.iter().enumerate() {
        let Some(name) = decl.name.as_ref() else {
            continue;
        };
        if name.starts_with("__") {
            continue;
        }
        vars.push(DebugVar {
            name: name.clone(),
            local: i as u32,
            global: vars.len() as u32,
            spill: spill_kind(interner, decl.ty),
            type_id: registry.intern(decl.ty),
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
