//! Decodes a paused thread's locals out of the wasm instance's linear memory, using the debug type
//! table (see [`sourcemap::TypeDesc`]) to expand strings, structs, unions, and arrays into DAP
//! `variables` trees. Split out of `mod.rs`.

use crate::execution::host::read_string_from_memory;
use std::collections::HashMap;
use std::sync::Arc;
use wasmtime::*;

use super::sourcemap::{self, ScalarKind, SourceMap, TypeDesc};
use super::state::{Shared, ThreadState, VarValue};

/// Maximum recursion depth when eagerly decoding an aggregate into an inline summary. Nested values
/// beyond this are collapsed to `{…}` but remain expandable on demand from the summary's children.
const MAX_DECODE_DEPTH: usize = 4;
/// Caps on how much of a container we materialize, to bound work on large/cyclic data.
const MAX_FIELDS: usize = 128;
const MAX_ELEMS: usize = 200;

/// Reads and decodes every named local of `thread_id`'s innermost frame from the spill-pool globals,
/// walking linear memory to expand strings, structs, unions, and arrays. Returns the thread's stop-time
/// `variablesReference -> children` map, keyed within the thread's range: the thread base holds the
/// top-frame locals, expandable children get base+1.. . An empty map means nothing to show.
pub(super) fn snapshot_locals(
    caller: &mut Caller<'_, ()>,
    shared: &Arc<Shared>,
    sm: &SourceMap,
    thread_id: u32,
) -> HashMap<i64, Vec<VarValue>> {
    let base = ThreadState::base_ref(thread_id);
    let func_id = {
        let inner = shared.inner.lock().unwrap();
        inner
            .threads
            .get(&thread_id)
            .and_then(|t| t.call_stack.last().map(|f| f.func_id))
    };
    let Some(info) = func_id.and_then(|id| sm.function(id)) else {
        return HashMap::new();
    };

    // Read the raw spilled slots first (needs `&mut caller`), then decode against memory.
    let raws: Vec<(String, u32, u64)> = info
        .vars
        .iter()
        .map(|v| {
            (
                v.name.clone(),
                v.type_id,
                read_global_i64(caller, v.global).unwrap_or(0) as u64,
            )
        })
        .collect();

    let Some(mem) = caller.get_export("memory").and_then(Extern::into_memory) else {
        return HashMap::new();
    };
    let mut dec = Decoder {
        caller: &*caller,
        mem,
        types: &sm.types,
        refs: HashMap::new(),
        next_ref: base + 1,
    };
    let mut locals = Vec::with_capacity(raws.len());
    for (name, type_id, raw) in raws {
        let (value, type_name, vref) = dec.build(type_id, raw, MAX_DECODE_DEPTH);
        locals.push(VarValue {
            name,
            value,
            type_name,
            variables_reference: vref,
        });
    }
    // The top-frame locals live under the thread's base reference (returned by `scopes`).
    dec.refs.insert(base, locals);
    dec.refs
}

/// Reads the raw `i64` bits spilled into the `$__dbg_v{global}` pool global.
fn read_global_i64(caller: &mut Caller<'_, ()>, global: u32) -> Option<i64> {
    let name = format!("__dbg_v{}", global);
    let g = caller.get_export(&name).and_then(Extern::into_global)?;
    match g.get(&mut *caller) {
        Val::I64(v) => Some(v),
        _ => None,
    }
}

/// Walks live values in linear memory against the debug type table, producing displayable strings and
/// a registry of expandable children keyed by `variablesReference`.
struct Decoder<'a, 'c> {
    caller: &'a Caller<'c, ()>,
    mem: Memory,
    types: &'a [TypeDesc],
    refs: HashMap<i64, Vec<VarValue>>,
    next_ref: i64,
}

impl Decoder<'_, '_> {
    fn desc(&self, type_id: u32) -> TypeDesc {
        self.types
            .get(type_id as usize)
            .cloned()
            .unwrap_or(TypeDesc::Ref)
    }

    /// Registers a child list and returns a fresh `variablesReference` for it (0 if empty).
    fn alloc(&mut self, children: Vec<VarValue>) -> i64 {
        if children.is_empty() {
            return 0;
        }
        let r = self.next_ref;
        self.next_ref += 1;
        self.refs.insert(r, children);
        r
    }

    fn u8_at(&self, addr: u32) -> u8 {
        let d = self.mem.data(self.caller);
        d.get(addr as usize).copied().unwrap_or(0)
    }

    fn u32_at(&self, addr: u32) -> u32 {
        let d = self.mem.data(self.caller);
        let a = addr as usize;
        if a + 4 <= d.len() {
            u32::from_le_bytes([d[a], d[a + 1], d[a + 2], d[a + 3]])
        } else {
            0
        }
    }

    fn u64_at(&self, addr: u32) -> u64 {
        let d = self.mem.data(self.caller);
        let a = addr as usize;
        if a + 8 <= d.len() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&d[a..a + 8]);
            u64::from_le_bytes(b)
        } else {
            0
        }
    }

    fn read_string(&self, ptr: u32) -> String {
        read_string_from_memory(&self.mem, self.caller, ptr as i32)
    }

    /// Loads a scalar's raw bits from memory at `addr`, honoring its storage width.
    fn load_scalar(&self, addr: u32, k: ScalarKind) -> u64 {
        match k.width() {
            1 => self.u8_at(addr) as u64,
            8 => self.u64_at(addr),
            _ => self.u32_at(addr) as u64,
        }
    }

    /// Computes the "raw" value for a field at `base + field.offset`: the scalar bits for a scalar, the
    /// inline address for a value aggregate, or the loaded pointer for a reference/string/array field.
    fn field_raw(&self, base: u32, field: &sourcemap::FieldDesc) -> u64 {
        let addr = base.wrapping_add(field.offset);
        match self.desc(field.type_id) {
            TypeDesc::Scalar(k) => self.load_scalar(addr, k),
            TypeDesc::Enum => self.u32_at(addr) as u64,
            TypeDesc::Str | TypeDesc::Array { .. } | TypeDesc::Ref => self.u32_at(addr) as u64,
            TypeDesc::Struct { value: true, .. } | TypeDesc::Union { value: true, .. } => {
                addr as u64
            }
            TypeDesc::Struct { .. } | TypeDesc::Union { .. } => self.u32_at(addr) as u64,
        }
    }

    /// Decodes a value of type `type_id` given its `raw` slot (scalar bits, or a data pointer in the
    /// low 32 bits for aggregates/strings). Returns `(display, type_name, variables_reference)`.
    fn build(&mut self, type_id: u32, raw: u64, depth: usize) -> (String, String, i64) {
        match self.desc(type_id) {
            TypeDesc::Scalar(k) => (decode_scalar(k, raw), k.tag().to_string(), 0),
            TypeDesc::Enum => ((raw as u32 as i32).to_string(), "enum".to_string(), 0),
            TypeDesc::Str => {
                let ptr = raw as u32;
                let val = if ptr == 0 {
                    "null".to_string()
                } else {
                    format!("\"{}\"", self.read_string(ptr))
                };
                (val, "string".to_string(), 0)
            }
            TypeDesc::Ref => {
                let low = raw as u32;
                let val = if low == 0 {
                    "null".to_string()
                } else {
                    format!("0x{:x}", low)
                };
                (val, "ref".to_string(), 0)
            }
            TypeDesc::Struct { name, fields, .. } => {
                self.build_struct(&name, &fields, raw as u32, depth)
            }
            TypeDesc::Union { name, variants, .. } => {
                self.build_union(&name, &variants, raw as u32, depth)
            }
            TypeDesc::Array { elem, stride } => self.build_array(elem, stride, raw as u32, depth),
        }
    }

    fn build_struct(
        &mut self,
        name: &str,
        fields: &[sourcemap::FieldDesc],
        ptr: u32,
        depth: usize,
    ) -> (String, String, i64) {
        if ptr == 0 {
            return ("null".to_string(), name.to_string(), 0);
        }
        if depth == 0 {
            return (format!("{} {{…}}", name), name.to_string(), 0);
        }
        let mut children = Vec::new();
        let mut parts = Vec::new();
        for field in fields.iter().take(MAX_FIELDS) {
            let raw = self.field_raw(ptr, field);
            let (value, type_name, vref) = self.build(field.type_id, raw, depth - 1);
            parts.push(format!("{}: {}", field.name, value));
            children.push(VarValue {
                name: field.name.clone(),
                value,
                type_name,
                variables_reference: vref,
            });
        }
        let summary = format!("{} {{ {} }}", name, parts.join(", "));
        let vref = self.alloc(children);
        (summary, name.to_string(), vref)
    }

    fn build_union(
        &mut self,
        name: &str,
        variants: &[sourcemap::VariantDesc],
        ptr: u32,
        depth: usize,
    ) -> (String, String, i64) {
        if ptr == 0 {
            return ("null".to_string(), name.to_string(), 0);
        }
        let disc = self.u32_at(ptr) as i32;
        let Some(variant) = variants.iter().find(|v| v.discriminant == disc) else {
            return (format!("{}(<tag {}>)", name, disc), name.to_string(), 0);
        };
        let type_name = format!("{}.{}", name, variant.name);
        if variant.fields.is_empty() {
            return (variant.name.clone(), type_name, 0);
        }
        if depth == 0 {
            return (format!("{}(…)", variant.name), type_name, 0);
        }
        let mut children = Vec::new();
        let mut parts = Vec::new();
        for field in variant.fields.iter().take(MAX_FIELDS) {
            let raw = self.field_raw(ptr, field);
            let (value, tn, vref) = self.build(field.type_id, raw, depth - 1);
            parts.push(value.clone());
            children.push(VarValue {
                name: field.name.clone(),
                value,
                type_name: tn,
                variables_reference: vref,
            });
        }
        let summary = format!("{}({})", variant.name, parts.join(", "));
        let vref = self.alloc(children);
        (summary, type_name, vref)
    }

    fn build_array(
        &mut self,
        elem: u32,
        stride: u32,
        ptr: u32,
        depth: usize,
    ) -> (String, String, i64) {
        if ptr == 0 {
            return ("null".to_string(), "array".to_string(), 0);
        }
        let len = self.u32_at(ptr);
        let elem_type = element_type_name(&self.desc(elem));
        let type_name = format!("{}[]", elem_type);
        if depth == 0 {
            return (format!("{}[{}]", elem_type, len), type_name, 0);
        }
        let shown = (len as usize).min(MAX_ELEMS);
        let mut children = Vec::new();
        for i in 0..shown {
            let addr = ptr + 4 + (i as u32) * stride;
            let raw = self.elem_raw(addr, elem);
            let (value, tn, vref) = self.build(elem, raw, depth - 1);
            children.push(VarValue {
                name: format!("[{}]", i),
                value,
                type_name: tn,
                variables_reference: vref,
            });
        }
        let summary = format!("{}[{}]", elem_type, len);
        let vref = self.alloc(children);
        (summary, type_name, vref)
    }

    /// The raw value of an array element stored at `addr` (mirrors [`Self::field_raw`]).
    fn elem_raw(&self, addr: u32, elem: u32) -> u64 {
        match self.desc(elem) {
            TypeDesc::Scalar(k) => self.load_scalar(addr, k),
            TypeDesc::Enum => self.u32_at(addr) as u64,
            TypeDesc::Str | TypeDesc::Array { .. } | TypeDesc::Ref => self.u32_at(addr) as u64,
            TypeDesc::Struct { value: true, .. } | TypeDesc::Union { value: true, .. } => {
                addr as u64
            }
            TypeDesc::Struct { .. } | TypeDesc::Union { .. } => self.u32_at(addr) as u64,
        }
    }
}

/// A short type label for an array element type, used to name the array (`int[]`, `Point[]`).
fn element_type_name(desc: &TypeDesc) -> String {
    match desc {
        TypeDesc::Scalar(k) => k.tag().to_string(),
        TypeDesc::Str => "string".to_string(),
        TypeDesc::Enum => "enum".to_string(),
        TypeDesc::Struct { name, .. } | TypeDesc::Union { name, .. } => name.clone(),
        TypeDesc::Array { .. } => "array".to_string(),
        TypeDesc::Ref => "ref".to_string(),
    }
}

/// Decodes a scalar's raw bits into its displayable form.
fn decode_scalar(kind: ScalarKind, raw: u64) -> String {
    let low = raw as u32;
    match kind {
        ScalarKind::Int => (low as i32).to_string(),
        ScalarKind::UInt => low.to_string(),
        ScalarKind::Byte => (low as u8).to_string(),
        ScalarKind::Bool => {
            if low != 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        ScalarKind::Char => char::from_u32(low)
            .map(|c| format!("'{}'", c))
            .unwrap_or_else(|| format!("\\u{{{:x}}}", low)),
        ScalarKind::Long => (raw as i64).to_string(),
        ScalarKind::ULong => raw.to_string(),
        ScalarKind::Float => f32::from_bits(low).to_string(),
        ScalarKind::Double => f64::from_bits(raw).to_string(),
    }
}
