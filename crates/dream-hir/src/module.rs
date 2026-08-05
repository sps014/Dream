//! HIR module-level containers: program, functions, globals, interfaces, imports.

use dream_types::{DefId, TypeId};
use crate::layout::LayoutTable;
use crate::nodes::{HExpr, HStmt};

/// A local variable slot within a function (parameters and `let`-bindings), unique per function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub u32);

/// A module-level (global) variable slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalId(pub u32);

/// An index into [`Hir::instances`] identifying one monomorphized instance of a generic def.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(pub u32);

/// A whole compiled program in HIR form.
#[derive(Debug, Default)]
pub struct Hir {
    /// Non-generic functions and already-monomorphized function bodies, in emission order.
    pub functions: Vec<HFunction>,
    /// Module-level variables.
    pub globals: Vec<HGlobal>,
    /// The monomorphization worklist: each entry is a concrete `(DefId, type-args)` instance the
    /// backend must emit. Populated as type-checking discovers generic uses.
    pub instances: Vec<MonoInstance>,
    /// Memory layout (field offsets/sizes) of every nominal type, so the backend can lower
    /// field/index access to concrete loads/stores.
    pub layouts: LayoutTable,
    /// Host/extern functions the module imports. The backend emits one `(import ...)` per entry;
    /// call sites resolve to `$name` (which the import declares).
    pub imports: Vec<HImport>,
    /// `@intrinsic("key")` externs: each maps a callee `DefId` to its intrinsic key. These have no
    /// emitted body — call sites resolve directly to the runtime helper `$<key>` (e.g. `string_alloc`)
    /// or, for async intrinsics like `sleep`, are recognized by the backend and lowered to the
    /// scheduler. Recorded so the backend's symbol table can resolve the callee def.
    pub intrinsics: Vec<(DefId, String)>,
    /// Interface dispatch metadata: the ordered interfaces (index = `iface_id`) and, per
    /// implementing class, the concrete method symbol for each `(interface, slot)`. Drives the
    /// itable data + dispatch trampolines emitted by the backend, and keeps concrete interface
    /// method implementations reachable through dead-code elimination.
    pub interfaces: InterfaceTable,
}

/// Interface dispatch metadata carried from analysis into codegen.
#[derive(Debug, Clone, Default)]
pub struct InterfaceTable {
    /// The program's interfaces in registration order; the index into this vector is the stable
    /// `iface_id` referenced by [`HExprKind::InterfaceCall`].
    pub interfaces: Vec<InterfaceInfo>,
    /// Every class that implements at least one interface, with the concrete method symbols it
    /// supplies for each implemented interface.
    pub impls: Vec<InterfaceImpl>,
}

/// One interface's dispatch shape: its method count and the interned `fun(this, params): ret`
/// signature of each method slot (used to declare the `call_indirect` type + trampoline).
#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub name: String,
    pub method_count: usize,
    /// The `call_indirect` signature (a `Func` `TypeId`) for each method slot.
    pub sigs: Vec<TypeId>,
}

/// One class's interface implementations: for each interface it implements, the concrete method
/// symbol (`{Class}_{method}`) that fills each method slot, keyed by the interface's `iface_id`.
#[derive(Debug, Clone)]
pub struct InterfaceImpl {
    /// The implementing class's interned struct type (its `struct_tags` key / runtime tag).
    pub class_ty: TypeId,
    /// `(iface_id, [concrete method symbol per slot])`.
    pub entries: Vec<(usize, Vec<String>)>,
}

/// A host function the module imports: an `extern fun` (interop) or a compiler-provided host
/// builtin (the `print_*` family). `module`/`field` name the WASM import target; `name` is the
/// internal symbol call sites reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HImport {
    /// The imported function's def, so call sites (which carry the callee `DefId`) resolve to this
    /// import's `$name` rather than the emitter's `$def{N}` fallback.
    pub def: DefId,
    pub name: String,
    pub module: String,
    pub field: String,
    pub params: Vec<TypeId>,
    pub ret: Option<TypeId>,
}

/// One monomorphized instance of a generic def, keyed by `(DefId, args)` — never a mangled string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoInstance {
    pub def: DefId,
    pub args: Vec<TypeId>,
}

#[derive(Debug)]
pub struct HGlobal {
    pub id: GlobalId,
    pub name: String,
    pub ty: TypeId,
    pub is_const: bool,
    pub init: Option<HExpr>,
}

#[derive(Debug, Clone)]
pub struct HFunction {
    pub def: DefId,
    /// The base (un-mangled) source name; the backend derives the emitted symbol from
    /// `(def, instance args)`.
    pub name: String,
    /// The instance args when this is a monomorphized body, empty otherwise.
    pub instance: Vec<TypeId>,
    pub params: Vec<HParam>,
    pub ret: TypeId,
    pub locals: Vec<HLocal>,
    pub body: Vec<HStmt>,
    pub is_async: bool,
    /// Absolute path of the source file this function was declared in. Carried for debug-info so the
    /// backend/source-map can attribute each `DebugLine` to the right file. `None` for synthesized
    /// functions (module init, tests) that have no originating source file.
    pub file: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HParam {
    pub local: LocalId,
    pub name: String,
    pub ty: TypeId,
    /// True for a `ref` parameter backed by a value-struct box (see
    /// `Analyzer::ref_box_type`/`docs/compiler/03-hir.md`): its MIR local must alias the caller's
    /// storage in place rather than take a private copy (`FunctionBuilder::new_ref_param`).
    pub is_ref: bool,
}

/// Declaration metadata for a function local (used by the backend to allocate slots and by RC
/// insertion to know which locals are references).
#[derive(Debug, Clone)]
pub struct HLocal {
    pub id: LocalId,
    pub name: String,
    pub ty: TypeId,
}
