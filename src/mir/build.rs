//! An ergonomic builder for MIR function bodies. Tracks the "current" block so lowering code can
//! emit straight-line statements and split control flow without manually indexing the block vector.

use super::{
    BasicBlock, BlockId, Local, LocalDecl, MirFunction, Place, Rvalue, Statement, Terminator,
};
use crate::types::{DefId, TypeId};

pub struct FunctionBuilder {
    name: String,
    ret: TypeId,
    is_async: bool,
    def: DefId,
    instance: Vec<TypeId>,
    params: Vec<Local>,
    locals: Vec<LocalDecl>,
    blocks: Vec<BasicBlock>,
    current: BlockId,
    file: Option<String>,
}

impl FunctionBuilder {
    /// Starts a function with a single empty entry block (block 0) as the current block. The def
    /// defaults to `DefId(0)`; lowering sets the real one via [`Self::set_def`].
    pub fn new(name: impl Into<String>, ret: TypeId) -> Self {
        FunctionBuilder {
            name: name.into(),
            ret,
            is_async: false,
            def: DefId(0),
            instance: Vec::new(),
            params: Vec::new(),
            locals: Vec::new(),
            blocks: vec![BasicBlock::default()],
            current: BlockId(0),
            file: None,
        }
    }

    pub fn set_async(&mut self, is_async: bool) {
        self.is_async = is_async;
    }

    /// Records the source file this function was declared in (debug-info line attribution).
    pub fn set_file(&mut self, file: Option<String>) {
        self.file = file;
    }

    /// Sets the nominal def and (optional) monomorphization instance args for the emitted symbol.
    pub fn set_def(&mut self, def: DefId, instance: Vec<TypeId>) {
        self.def = def;
        self.instance = instance;
    }

    /// Declares a local with an optional source name and returns its handle.
    pub fn new_local(&mut self, ty: TypeId, name: Option<String>) -> Local {
        let id = Local(self.locals.len() as u32);
        self.locals.push(LocalDecl { ty, name });
        id
    }

    /// Declares a synthetic, unnamed temporary.
    pub fn new_temp(&mut self, ty: TypeId) -> Local {
        self.new_local(ty, None)
    }

    /// Declares a local and records it as a parameter (parameters must be declared in order).
    pub fn new_param(&mut self, ty: TypeId, name: Option<String>) -> Local {
        let l = self.new_local(ty, name);
        self.params.push(l);
        l
    }

    /// Allocates a fresh, empty block (terminator defaults to `Unreachable` until set).
    pub fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock::default());
        id
    }

    pub fn current(&self) -> BlockId {
        self.current
    }

    pub fn switch_to(&mut self, block: BlockId) {
        self.current = block;
    }

    /// Appends a statement to the current block.
    pub fn push(&mut self, stmt: Statement) {
        self.blocks[self.current.0 as usize].stmts.push(stmt);
    }

    /// Convenience for `place = rvalue`.
    pub fn assign(&mut self, place: Place, rvalue: Rvalue) {
        self.push(Statement::Assign(place, rvalue));
    }

    /// Sets the terminator of the current block (overwrites the default).
    pub fn terminate(&mut self, terminator: Terminator) {
        self.blocks[self.current.0 as usize].terminator = terminator;
    }

    /// True if the current block already has a real (non-default) terminator. Lowering uses this to
    /// avoid emitting dead fall-through gotos after a `return`.
    pub fn is_terminated(&self) -> bool {
        !matches!(
            self.blocks[self.current.0 as usize].terminator,
            Terminator::Unreachable
        )
    }

    pub fn local_ty(&self, local: Local) -> TypeId {
        self.locals[local.0 as usize].ty
    }

    pub fn finish(self) -> MirFunction {
        MirFunction {
            def: self.def,
            instance: self.instance,
            name: self.name,
            ret: self.ret,
            is_async: self.is_async,
            params: self.params,
            locals: self.locals,
            blocks: self.blocks,
            entry: BlockId(0),
            hir_fn: None,
            file: self.file,
        }
    }
}
