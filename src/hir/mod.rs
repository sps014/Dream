//! The typed, name-resolved High-level IR (HIR).
//!
//! The analyzer lowers the AST to HIR after type-checking, recording everything codegen used to
//! re-derive: every expression carries a [`TypeId`]; every variable reference is a resolved
//! [`Binding`]; every call names a resolved [`Callee`] (def + chosen overload + monomorphization
//! instance). Control flow is still structured here (if/while/for/switch) — desugaring into a
//! CFG happens in the MIR . Monomorphization is recorded as an explicit
//! [`MonoInstance`] worklist instead of being rediscovered from mangled names.

pub mod layout;
pub mod ops;

pub use layout::{scalar_size, FieldLayout, LayoutTable, TypeLayout, UnionLayout, UnionVariant};
pub use ops::{BinOp, UnOp};

use crate::types::{DefId, TypeId};

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
}

/// Declaration metadata for a function local (used by the backend to allocate slots and by RC
/// insertion to know which locals are references).
#[derive(Debug, Clone)]
pub struct HLocal {
    pub id: LocalId,
    pub name: String,
    pub ty: TypeId,
}

/// A resolved reference to a variable or function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Local(LocalId),
    Global(GlobalId),
    Func(Callee),
}

/// A fully resolved call target: the def, the monomorphization type-args (empty when non-generic),
/// and the concrete return type at this call site. The backend derives the emitted symbol from
/// `(def, instance)`, matching the instance function's own `(def, instance)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callee {
    pub def: DefId,
    pub instance: Vec<TypeId>,
    pub ret: TypeId,
}

/// Structured statements. Control flow is preserved (lowered to a CFG only in MIR).
#[derive(Debug, Clone)]
pub enum HStmt {
    /// `let name: ty = value;`
    Let {
        local: LocalId,
        ty: TypeId,
        value: HExpr,
    },
    /// Assignment to a place (local/global/field/index).
    Assign {
        place: HPlace,
        value: HExpr,
    },
    /// An expression evaluated for its effect.
    Expr(HExpr),
    Return(Option<HExpr>),
    If {
        cond: HExpr,
        then_branch: Vec<HStmt>,
        else_branch: Vec<HStmt>,
    },
    While {
        cond: HExpr,
        body: Vec<HStmt>,
        /// Enclosing loop label (`outer: while ...`), targetable by `break`/`continue label`.
        label: Option<String>,
    },
    /// A `do { body } while (cond)` loop: the body runs once before the condition is first tested.
    DoWhile {
        cond: HExpr,
        body: Vec<HStmt>,
        label: Option<String>,
    },
    /// A counted/`for` loop with an explicit init/cond/step (already desugared from surface syntax
    /// far enough to carry typed parts).
    For {
        init: Box<HStmt>,
        cond: HExpr,
        step: Box<HStmt>,
        body: Vec<HStmt>,
        label: Option<String>,
    },
    /// `foreach (elem in iterable)`: the iterable yields an array of `elem`'s type.
    Foreach {
        elem: LocalId,
        iterable: HExpr,
        body: Vec<HStmt>,
        label: Option<String>,
    },
    /// A `switch` over a scrutinee (both the C-style and pattern-matching forms lower here). Each
    /// arm is a typed pattern + body; `default` runs when no arm matches.
    Switch {
        scrutinee: HExpr,
        arms: Vec<HArm>,
        default: Vec<HStmt>,
    },
    Break(Option<String>),
    Continue(Option<String>),
    /// `await e;` at statement position (the only legal await position).
    Await(HExpr),
    /// A debug-info marker recording the 1-based source line of the *next* executable statement.
    /// Emitted only when the compiler runs with debug-info enabled; lowers to a `Statement::DebugLine`
    /// in MIR and, ultimately, a call to the host `dream_debug.line` hook. Carries no runtime value.
    DebugLine(u32),
}

/// One arm of a `switch`.
#[derive(Debug, Clone)]
pub struct HArm {
    pub pattern: HPattern,
    pub body: Vec<HStmt>,
}

/// Switch patterns. Union-variant patterns bind their payload to fresh locals.
#[derive(Debug, Clone)]
pub enum HPattern {
    /// A constant value (enum member, integer, etc.).
    Const(HExpr),
    /// A union variant `Variant(bindings...)` of the union `def`.
    Variant {
        def: DefId,
        variant: usize,
        bindings: Vec<LocalId>,
    },
    /// `_` wildcard.
    Wildcard,
}

/// An assignable location.
#[derive(Debug, Clone)]
pub enum HPlace {
    Local(LocalId),
    Global(GlobalId),
    /// `obj.field` — `field` is the resolved field index in the struct layout.
    Field {
        obj: Box<HExpr>,
        field: usize,
    },
    /// `array[index]`.
    Index {
        array: Box<HExpr>,
        index: Box<HExpr>,
    },
}

/// A typed expression: `kind` is the shape, `ty` is its interned result type.
#[derive(Debug, Clone)]
pub struct HExpr {
    pub ty: TypeId,
    pub kind: HExprKind,
}

#[derive(Debug, Clone)]
pub enum HExprKind {
    IntLit(i64),
    FloatLit(f64),
    BoolLit(bool),
    CharLit(char),
    StringLit(String),
    /// The `null` literal (typed as `T?` at its use site).
    Null,
    /// A resolved variable read.
    Var(Binding),
    Binary {
        op: BinOp,
        lhs: Box<HExpr>,
        rhs: Box<HExpr>,
    },
    Unary {
        op: UnOp,
        operand: Box<HExpr>,
    },
    /// A direct function call to a resolved callee.
    Call {
        callee: Callee,
        args: Vec<HExpr>,
    },
    /// A method call `obj.method(args)`; resolved to a callee plus the receiver.
    MethodCall {
        receiver: Box<HExpr>,
        callee: Callee,
        args: Vec<HExpr>,
    },
    /// An indirect call through a function-typed value.
    IndirectCall {
        target: Box<HExpr>,
        args: Vec<HExpr>,
    },
    /// A dynamically-dispatched interface method call `receiver.method(args)` where `receiver`'s
    /// static type is an interface. Dispatch happens at runtime through the receiver's concrete
    /// tag (tag-indexed itable + `call_indirect`); `iface_id` is the interface's stable id,
    /// `method_slot` the method's local index within that interface, and `sig` the interned
    /// function type `fun(this, params...): ret` used for the indirect call.
    InterfaceCall {
        receiver: Box<HExpr>,
        iface_id: usize,
        method_slot: usize,
        sig: TypeId,
        args: Vec<HExpr>,
    },
    /// Constructor `Type(args)`; `instance` records the monomorphization type-args when generic.
    /// When `ctor` is `Some`, `args` are the user `constructor(){}`'s arguments (the backend
    /// allocates, then calls that constructor with `this` + args); when `None`, the implicit zero-arg
    /// default constructor takes no args and every field is zero-initialized.
    New {
        def: DefId,
        instance: Vec<TypeId>,
        ctor: Option<DefId>,
        args: Vec<HExpr>,
    },
    /// Union variant construction `Union.Variant(args)`.
    UnionNew {
        def: DefId,
        variant: usize,
        args: Vec<HExpr>,
    },
    /// `obj.field` read; `field` is the resolved field index.
    Field {
        obj: Box<HExpr>,
        field: usize,
    },
    /// `array[index]` read.
    Index {
        array: Box<HExpr>,
        index: Box<HExpr>,
    },
    /// `array.len()` — the length word stored at the array's data pointer.
    ArrayLen(Box<HExpr>),
    /// `string.len()` — a runtime `$strlen` call (O(1): strings are length-prefixed, the length word
    /// is stored at the data pointer, mirroring arrays).
    StrLen(Box<HExpr>),
    /// `string.char_at(i)` — a runtime `$char_at` read: `.0` is the string, `.1` the byte index.
    CharAt(Box<HExpr>, Box<HExpr>),
    /// The object-protocol `x.hash_code()` (typed `int`) with no user override: dispatches on the
    /// receiver's static type to the matching hash helper.
    HashCode(Box<HExpr>),
    /// The object-protocol `x.to_string()` (typed `string`) with no user override: dispatches on the
    /// receiver's static type to the matching `$*_to_string` helper.
    ToString(Box<HExpr>),
    /// String concatenation `a + b` (both operands already string-typed, non-string operands wrapped
    /// in [`HExprKind::ToString`]): joins the two via the runtime `$concat_strings`.
    Concat(Box<HExpr>, Box<HExpr>),
    /// A C-style enum's `to_string()` — maps the operand's discriminant to its interned variant-name
    /// string. `arms` is `(discriminant, variant name)` for every member; an unmatched value yields `""`.
    EnumName {
        value: Box<HExpr>,
        arms: Vec<(i64, String)>,
    },
    /// `Buffer.alloc<T>(len)` — a zero-initialized `T[]` of a runtime length. `elem_ty` is the element
    /// type; `len` the element count.
    ArrayNew {
        elem_ty: TypeId,
        len: Box<HExpr>,
    },
    ArrayLit {
        elem_ty: TypeId,
        elems: Vec<HExpr>,
    },
    /// An explicit or implicit numeric/object coercion to `ty`.
    Cast(Box<HExpr>),
    /// `cond ? then : else_`.
    Ternary {
        cond: Box<HExpr>,
        then_expr: Box<HExpr>,
        else_expr: Box<HExpr>,
    },
    /// Null-coalescing `lhs ?? rhs`.
    Coalesce {
        lhs: Box<HExpr>,
        rhs: Box<HExpr>,
    },
    /// `await e` used as a value (only valid in the limited await positions; carries the awaited
    /// future's inner type as `ty`).
    Await(Box<HExpr>),
    /// An enum member reference resolved to its integer value.
    EnumValue(i64),
    /// Reads a union value's discriminant (the `i32` variant tag). Emitted when a guarded/nested
    /// pattern `switch` is lowered to an if-chain instead of a `Switch`.
    Discriminant(Box<HExpr>),
    /// Reads payload field `field` of `variant` from union value `base` (whose interned union type is
    /// `union_ty`). Only valid once the discriminant is known to select `variant`.
    UnionField {
        base: Box<HExpr>,
        union_ty: TypeId,
        variant: usize,
        field: usize,
    },
    /// A runtime type test `value is target` on an `object`-typed value (typed `bool`): a runtime tag
    /// comparison. Concrete-typed operands are folded to a `BoolLit` in the analyzer instead.
    IsType {
        value: Box<HExpr>,
        target: TypeId,
    },
    /// The `print`/`println` builtins (`System.print`/`System.println`), lowered to the host
    /// `print_*` imports. Void-typed; only valid in statement position. `newline` appends a `\n`.
    Print {
        arg: Box<HExpr>,
        newline: bool,
    },
    /// A dynamic `js` call whose variadic arguments are marshaled through the shadow stack in a
    /// single host crossing (no per-argument boxing, no heap array). `callee` is the bridge import
    /// (`js.__call` for a method call, `js.__invoke` for calling the value); `target` is the JS
    /// receiver/callee handle; `method` is the method-name string for `target[name](...)` (or `None`
    /// to call `target(...)`); `args` are the raw arguments (each with its type, which selects the
    /// tagged-slot layout). Result is `js`.
    JsCall {
        callee: Callee,
        target: Box<HExpr>,
        method: Option<Box<HExpr>>,
        args: Vec<HExpr>,
    },
}

impl HExpr {
    pub fn new(ty: TypeId, kind: HExprKind) -> Self {
        HExpr { ty, kind }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DefKind, TypeCtx};

    #[test]
    fn can_build_a_small_typed_hir() {
        let mut ctx = TypeCtx::new();
        let def = ctx.register(DefKind::Function, "add", vec![]);
        let int = ctx.interner.int();

        // fun add(a: int, b: int): int { return a + b; }
        let body = vec![HStmt::Return(Some(HExpr::new(
            int,
            HExprKind::Binary {
                op: BinOp::Add,
                lhs: Box::new(HExpr::new(int, HExprKind::Var(Binding::Local(LocalId(0))))),
                rhs: Box::new(HExpr::new(int, HExprKind::Var(Binding::Local(LocalId(1))))),
            },
        )))];

        let func = HFunction {
            def,
            name: "add".to_string(),
            instance: vec![],
            params: vec![
                HParam {
                    local: LocalId(0),
                    name: "a".into(),
                    ty: int,
                },
                HParam {
                    local: LocalId(1),
                    name: "b".into(),
                    ty: int,
                },
            ],
            ret: int,
            locals: vec![],
            body,
            is_async: false,
            file: None,
        };

        let hir = Hir {
            functions: vec![func],
            globals: vec![],
            instances: vec![],
            layouts: LayoutTable::default(),
            imports: vec![],
            intrinsics: vec![],
            interfaces: InterfaceTable::default(),
        };
        assert_eq!(hir.functions.len(), 1);
        assert_eq!(hir.functions[0].params.len(), 2);
        assert!(matches!(hir.functions[0].body[0], HStmt::Return(Some(_))));
    }
}
