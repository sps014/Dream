//! Interleaved HIR emission.
//!
//! As the analyzer type-checks a function it *also* builds the typed, name-resolved
//! [`crate::hir`] for it — the single-source-of-truth approach: there is no second type inference
//! pass. Each expression records its [`HExpr`] into [`HirEmit::last`] (a side-channel that avoids
//! threading a return value through the ~50 `analyze_expression` call sites) and each statement
//! appends an [`HStmt`]. A function is emitted only if *every* construct in it is representable;
//! anything unrepresentable flips [`HirEmit::ok`] to `false` and the function is skipped (it then has
//! no backend output). The HIR is the only input the backend consumes.

use super::Analyzer;
use crate::hir::{
    BinOp, Binding, Callee, GlobalId, HArm, HExpr, HExprKind, HFunction, HGlobal, HImport, HLocal,
    HParam, HPattern, HPlace, HStmt, LocalId, UnOp,
};
use crate::syntax::nodes::{FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::types::{DefId, DefKind, PrimTy, TyKind, TypeId};
use indexmap::IndexMap;

mod build;
mod exprs;
mod stmts;

/// Per-analysis HIR-emission state, plus the accumulated [`HFunction`]s. Reset at the start of each
/// candidate function (see [`Analyzer::hir_begin_function`]).
#[derive(Default)]
pub(super) struct HirEmit {
    /// True while inside a function we are attempting to emit. When false, every helper is a no-op,
    /// so non-candidate functions (generic templates, methods, anything unsupported) cost nothing.
    collecting: bool,
    /// True while every construct seen in the current function has been representable in HIR. Once
    /// false, the function will not be emitted.
    ok: bool,
    /// The HIR of the most-recently-analyzed expression (`None` if it was not representable). A
    /// parent expression takes this immediately after analyzing each child.
    last: Option<HExpr>,
    /// Name -> (slot, type) for the current function's locals (parameters first, then `let`s).
    /// Keyed by name, so a re-declaration (shadowing in a sibling/nested scope) overwrites the entry;
    /// unique slot ids therefore come from `next_local`, not this map's length.
    locals: IndexMap<String, (LocalId, TypeId)>,
    /// Monotonic allocator for local slot ids. Incremented for every parameter and `let`, so shadowed
    /// names never collide on a slot (which would merge distinct-typed locals into one).
    next_local: u32,
    local_decls: Vec<HLocal>,
    params: Vec<HParam>,
    /// Stack of statement lists being built. The bottom is the function body; control-flow handlers
    /// push a frame for each nested block and pop it to attach to the enclosing statement.
    blocks: Vec<Vec<HStmt>>,
    def: Option<DefId>,
    name: String,
    /// The monomorphization type-args of the function currently being emitted (empty for a plain,
    /// non-generic function). Together with `def` this determines the emitted symbol, so a generic
    /// instance body and its call sites agree.
    instance: Vec<TypeId>,
    ret: Option<TypeId>,
    is_async: bool,
    /// Name -> (slot, type) for module-level variables, populated once after globals are analyzed
    /// (see [`Analyzer::hir_register_globals`]). Read by identifier/assignment lowering so a name
    /// that is not a local resolves to a [`Binding::Global`].
    globals: IndexMap<String, (GlobalId, TypeId)>,
    /// Captured global initializer expressions, keyed by variable name, attached to the matching
    /// [`HGlobal`] in [`Analyzer::hir_register_globals`]. Populated while top-level variables are
    /// analyzed (see [`Analyzer::hir_global_init_begin`]).
    pending_global_inits: IndexMap<String, HExpr>,
    /// All successfully emitted functions, surfaced via `SemanticInfo::hir`.
    pub functions: Vec<HFunction>,
    /// The module-global declarations, surfaced via `SemanticInfo::hir`.
    pub global_decls: Vec<HGlobal>,
}

/// Maps a surface binary operator token to the IR operator, or `None` for operators not yet lowered
/// by the interleaved emitter (short-circuiting `&&`/`||` and `??`, which desugar to control flow).
fn token_to_binop(kind: TokenKind) -> Option<BinOp> {
    Some(match kind {
        TokenKind::PlusToken => BinOp::Add,
        TokenKind::MinusToken => BinOp::Sub,
        TokenKind::StarToken => BinOp::Mul,
        TokenKind::SlashToken => BinOp::Div,
        TokenKind::ModulusToken => BinOp::Rem,
        TokenKind::EqualEqualToken => BinOp::Eq,
        TokenKind::NotEqualToken => BinOp::Ne,
        TokenKind::GreaterThanToken => BinOp::Gt,
        TokenKind::GreaterThanEqualToken => BinOp::Ge,
        TokenKind::SmallerThanToken => BinOp::Lt,
        TokenKind::SmallerThanEqualToken => BinOp::Le,
        TokenKind::BitWiseAmpersandToken => BinOp::BitAnd,
        TokenKind::BitWisePipeToken => BinOp::BitOr,
        TokenKind::BitWiseXorToken => BinOp::BitXor,
        TokenKind::ShiftLeftToken => BinOp::Shl,
        TokenKind::ShiftRightToken => BinOp::Shr,
        // Short-circuiting connectives: the MIR lowerer materializes these as branches
        // (`lower_short_circuit`), so they never reach the backend as a plain binary op.
        TokenKind::AmpersandAmpersandToken => BinOp::And,
        TokenKind::PipePipeToken => BinOp::Or,
        _ => return None,
    })
}

impl<'a> Analyzer<'a> {
    /// Starts HIR collection for `function`, returning whether it is a candidate. Slice 1 emits only
    /// plain non-generic, non-static free functions (no `this` receiver) that are registered as a
    /// `DefId`; everything else is skipped (collection stays off).
    pub(in crate::semantics::analyzer) fn hir_begin_function(&mut self, function: &FunctionNode<'a>) {
        // `extern` functions are declarations with no body: host-interop imports are emitted as
        // `(import ...)` (see `hir_build_imports`) and `@intrinsic` ones lower straight to their
        // runtime helper (e.g. `String.alloc` → `$string_alloc`). Emitting an (empty) HIR body for
        // them would define a second `$string_alloc`, colliding with the runtime function.
        if function.is_extern {
            self.hir.collecting = false;
            return;
        }
        let is_generic = function
            .generic_parameters
            .as_ref()
            .is_some_and(|p| !p.is_empty());
        // Methods are registered (and looked up here) under their mangled `{Type}_{method}` name;
        // `this` is simply parameter 0. Static methods have no receiver. Both are emittable. A free
        // function is registered under its *emitted* name (signature-mangled when overloaded), so an
        // overloaded declaration resolves to its own distinct `DefId` rather than a shared base def.
        let param_types: Vec<String> = function
            .parameters
            .iter()
            .map(|p| p.type_.get_type())
            .collect();
        let lookup_name = self
            .function_table
            .resolve_emitted_name(&function.name.text, &param_types, &mut self.type_ctx);
        let def = self
            .type_ctx
            .defs
            .lookup(DefKind::Function, &lookup_name);

        // A generic template is emitted once per monomorphization: the initial (unbound) pass is
        // skipped, and each concrete instantiation is analyzed again under `current_generic_bindings`
        // (see `analyze_pending_instantiations`). Anything with no registered def is skipped.
        let under_mono = !self.current_generic_bindings.is_empty();
        if def.is_none() || (is_generic && !under_mono) {
            self.hir.collecting = false;
            return;
        }
        // The instance type-args disambiguate the emitted symbol, but *only* for defs whose name is
        // shared across instantiations — i.e. generic free functions/methods, registered under their
        // base name. A method on a generic struct (`Box<int>.get`) is a non-generic method whose
        // specialization is already baked into its mangled `{Type_args}_{method}` def name, so it
        // takes an empty instance (its call sites resolve to that same mangled name with no suffix).
        let instance: Vec<TypeId> = if is_generic && under_mono {
            let concrete: Vec<Type> =
                self.current_generic_bindings.values().cloned().collect();
            concrete.iter().map(|c| self.type_ctx.lower(c)).collect()
        } else {
            Vec::new()
        };

        self.hir.collecting = true;
        self.hir.ok = true;
        self.hir.last = None;
        self.hir.locals.clear();
        self.hir.next_local = 0;
        self.hir.local_decls.clear();
        self.hir.params.clear();
        self.hir.blocks.clear();
        self.hir.blocks.push(Vec::new());
        self.hir.def = def;
        self.hir.instance = instance;
        self.hir.name = lookup_name;
        self.hir.is_async = function.is_async;
        self.hir.ret = Some(
            function
                .return_type
                .as_ref()
                .map(|t| self.type_ctx.lower(t))
                .unwrap_or_else(|| self.type_ctx.interner.void()),
        );

        for param in function.parameters.iter() {
            let ty = self.type_ctx.lower(&param.type_);
            let local = LocalId(self.hir.next_local);
            self.hir.next_local += 1;
            self.hir
                .locals
                .insert(param.name.text.clone(), (local, ty));
            self.hir.params.push(HParam {
                local,
                name: param.name.text.clone(),
                ty,
            });
        }
    }

    /// Finishes the current function: if it was a fully-supported candidate, builds and records its
    /// [`HFunction`]. Always turns collection back off.
    pub(in crate::semantics::analyzer) fn hir_finish_function(&mut self) {
        // A well-formed function leaves exactly the body frame on the stack; a mismatch means an
        // unbalanced push/pop, so refuse to emit rather than emit a truncated body.
        if self.hir.collecting && self.hir.ok && self.hir.blocks.len() == 1 {
            if let (Some(def), Some(ret)) = (self.hir.def, self.hir.ret) {
                let body = self.hir.blocks.pop().unwrap_or_default();
                self.hir.functions.push(HFunction {
                    def,
                    name: std::mem::take(&mut self.hir.name),
                    instance: std::mem::take(&mut self.hir.instance),
                    params: std::mem::take(&mut self.hir.params),
                    ret,
                    locals: std::mem::take(&mut self.hir.local_decls),
                    body,
                    is_async: self.hir.is_async,
                });
            }
        }
        self.hir.blocks.clear();
        self.hir.collecting = false;
    }

    /// Takes the HIR recorded for the most-recently-analyzed expression.
    pub(in crate::semantics::analyzer) fn hir_take(&mut self) -> Option<HExpr> {
        self.hir.last.take()
    }

    /// Marks the most-recent expression as not representable in HIR (clears `last`).
    pub(in crate::semantics::analyzer) fn hir_none(&mut self) {
        self.hir.last = None;
    }

    /// True while HIR is being collected for an emittable function. Lets sibling analyzer modules
    /// (e.g. the `js` interop desugar) skip emission work for non-candidate functions.
    pub(in crate::semantics::analyzer) fn hir_active(&self) -> bool {
        self.active()
    }

    /// Records `expr` as the most-recently-analyzed expression's HIR (or clears it when inactive).
    /// Used by desugars that build an [`HExpr`] directly rather than through a dedicated `hir_set_*`.
    pub(in crate::semantics::analyzer) fn hir_set_last(&mut self, expr: Option<HExpr>) {
        if !self.active() {
            self.hir.last = None;
            return;
        }
        self.hir.last = expr;
    }

    /// Flags the current function as not emittable (an unsupported construct was reached).
    pub(in crate::semantics::analyzer) fn hir_fail(&mut self) {
        if self.hir.collecting {
            self.hir.ok = false;
        }
    }

    fn active(&self) -> bool {
        self.hir.collecting && self.hir.ok
    }

    /// Appends a statement to the current (innermost) block, if collection is active.
    fn push_stmt(&mut self, stmt: HStmt) {
        if self.active() {
            if let Some(block) = self.hir.blocks.last_mut() {
                block.push(stmt);
            }
        }
    }

    /// Appends a fully-built statement to the current block (used by callers that assemble their own
    /// `HStmt`, e.g. the `if`/`else if` chain folder). Gated on the active flag like [`Self::push_stmt`].
    pub(in crate::semantics::analyzer) fn hir_push_stmt(&mut self, stmt: HStmt) {
        self.push_stmt(stmt);
    }

    /// Opens a nested statement block (e.g. a loop body). Paired with [`Self::hir_close_block`].
    /// Gated on `collecting` (not `ok`) so push/pop stay balanced even after the function is doomed.
    pub(in crate::semantics::analyzer) fn hir_open_block(&mut self) {
        if self.hir.collecting {
            self.hir.blocks.push(Vec::new());
        }
    }

    /// Closes the innermost block and returns its statements.
    pub(in crate::semantics::analyzer) fn hir_close_block(&mut self) -> Vec<HStmt> {
        if self.hir.collecting {
            self.hir.blocks.pop().unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    /// Allocates a fresh local slot without emitting a `let` (for loop-bound variables like a
    /// `foreach` element). Returns the slot, or `None` if collection is inactive.
    pub(in crate::semantics::analyzer) fn hir_alloc_local(&mut self, name: &str, ty: &Type) -> Option<LocalId> {
        self.alloc_local(name, ty)
    }

    fn alloc_local(&mut self, name: &str, ty: &Type) -> Option<LocalId> {
        if !self.active() {
            return None;
        }
        let ty = self.type_ctx.lower(ty);
        let local = LocalId(self.hir.next_local);
        self.hir.next_local += 1;
        self.hir.locals.insert(name.to_string(), (local, ty));
        self.hir.local_decls.push(HLocal {
            id: local,
            name: name.to_string(),
            ty,
        });
        Some(local)
    }
}

fn extern_import_target(func: &FunctionNode) -> (String, String) {
    let mut module = "env".to_string();
    let mut field = func.name.text.clone();
    if let Some(js) = func.attributes.iter().find(|a| a.name.text == "js") {
        if let Some(arg) = js.args.first() {
            module = arg.text.trim_matches('"').to_string();
        }
        if let Some(arg) = js.args.get(1) {
            field = arg.text.trim_matches('"').to_string();
        }
    }
    (module, field)
}

/// Expands the backslash escapes a string/char literal body may contain (`\n`, `\t`, `\r`, `\0`,
/// `\\`, `\"`, `\'`). Unknown escapes keep the escaped character verbatim, matching the lexer's
/// permissive stance.
fn unescape_lit_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The runtime content of a string literal: the raw token text still carries its surrounding double
/// quotes (it is the source slice), so strip them and expand escapes. Idempotent on already-unquoted
/// input.
fn string_lit_value(text: &str) -> String {
    let body = text.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(text);
    unescape_lit_body(body)
}
