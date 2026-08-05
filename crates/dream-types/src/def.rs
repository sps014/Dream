//! Definition tables: the [`DefTable`] assigns a stable [`DefId`] to every nominal declaration
//! (struct, union, enum, function) so interned types and the future HIR can reference declarations
//! by index instead of by mangled string name.

use super::DefId;
use indexmap::IndexMap;

/// What a [`DefId`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    Struct,
    Union,
    Enum,
    Function,
    Interface,
}

/// A single nominal declaration. `generic_params` records the declared type-parameter names (e.g.
/// `["T", "V"]`) in order; it is empty for non-generic defs. `name` is the source-level base name
/// (never a mangled monomorphization name).
#[derive(Debug, Clone)]
pub struct DefInfo {
    pub kind: DefKind,
    pub name: String,
    pub generic_params: Vec<String>,
    /// True for `struct` (value) declarations: instances are stored inline with copy semantics
    /// rather than as heap-allocated, reference-counted objects. Always `false` for non-structs.
    pub is_value: bool,
}

/// Interns nominal declarations to [`DefId`]s. Lookups by `(kind, name)` are deduplicated so a base
/// name maps to exactly one def; monomorphized instances are *not* separate defs (they are the same
/// `DefId` with different type arguments in [`TyKind::Struct`](super::TyKind)).
#[derive(Debug, Default)]
pub struct DefTable {
    defs: Vec<DefInfo>,
    by_name: IndexMap<(DefKind, String), DefId>,
}

impl DefTable {
    pub fn new() -> Self {
        DefTable::default()
    }

    /// Interns `(kind, name)`, returning the existing `DefId` if already present. `generic_params`
    /// is recorded on first insertion only.
    pub fn intern(&mut self, kind: DefKind, name: &str, generic_params: Vec<String>) -> DefId {
        let key = (kind, name.to_string());
        if let Some(&id) = self.by_name.get(&key) {
            return id;
        }
        let id = DefId(self.defs.len() as u32);
        self.defs.push(DefInfo {
            kind,
            name: name.to_string(),
            generic_params,
            is_value: false,
        });
        self.by_name.insert(key, id);
        id
    }

    /// Marks a definition as a value (`struct`) type. Idempotent.
    pub fn mark_value(&mut self, id: DefId) {
        self.defs[id.0 as usize].is_value = true;
    }

    /// True when `id` names a value (`struct`) type.
    pub fn is_value(&self, id: DefId) -> bool {
        self.defs[id.0 as usize].is_value
    }

    pub fn get(&self, id: DefId) -> &DefInfo {
        &self.defs[id.0 as usize]
    }

    pub fn lookup(&self, kind: DefKind, name: &str) -> Option<DefId> {
        self.by_name.get(&(kind, name.to_string())).copied()
    }

    pub fn name(&self, id: DefId) -> &str {
        &self.defs[id.0 as usize].name
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
}
