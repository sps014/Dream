//! The MIR optimization pass manager and passes.

mod algebraic;
mod const_fold;
mod dce;
mod gvn;
mod inline;
mod prop;
mod rc;
mod simplify_cfg;

pub use algebraic::Algebraic;
pub use const_fold::ConstFold;
pub(crate) use dce::is_pure;
pub use dce::Dce;
pub use gvn::Gvn;
pub use inline::Inliner;
pub use prop::CopyConstProp;
pub use rc::{RcElision, RcInsertion};
pub use simplify_cfg::SimplifyCfg;

use super::{Mir, MirFunction};
use crate::types::TypeInterner;

/// A single function-level MIR transformation.
pub trait MirPass {
    fn name(&self) -> &'static str;
    /// Runs the pass over one function. Returns `true` if it changed anything (drives the
    /// fixpoint loop in [`PassManager::run`]).
    fn run(&self, func: &mut MirFunction, interner: &TypeInterner) -> bool;
}

/// A whole-program transformation (needs to see every function at once, e.g. inlining). Distinct
/// from [`MirPass`], which is function-local.
pub trait ModulePass {
    fn name(&self) -> &'static str;
    /// Runs the pass over the whole module. Returns `true` if it changed anything.
    fn run(&self, mir: &mut Mir, interner: &TypeInterner) -> bool;
}

/// Runs a configured pipeline of passes to a fixpoint over each function.
pub struct PassManager {
    passes: Vec<Box<dyn MirPass>>,
    max_iterations: usize,
}

impl PassManager {
    pub fn new() -> Self {
        PassManager {
            passes: Vec::new(),
            max_iterations: 16,
        }
    }

    /// The default optimization pipeline, ordered so cheap simplifications expose work for the
    /// later ones (prop -> fold -> algebraic -> gvn -> simplify-cfg -> dce, then RC elision).
    pub fn default_pipeline() -> Self {
        let mut pm = PassManager::new();
        pm.add(CopyConstProp);
        pm.add(ConstFold);
        pm.add(Algebraic);
        pm.add(Gvn);
        pm.add(SimplifyCfg);
        pm.add(Dce);
        pm.add(RcElision);
        pm
    }

    pub fn add(&mut self, pass: impl MirPass + 'static) {
        self.passes.push(Box::new(pass));
    }

    /// Runs every pass repeatedly until none reports a change (or the iteration cap is hit).
    pub fn run(&self, func: &mut MirFunction, interner: &TypeInterner) {
        for _ in 0..self.max_iterations {
            let mut changed = false;
            for pass in &self.passes {
                changed |= pass.run(func, interner);
            }
            if !changed {
                break;
            }
        }
    }
}

impl Default for PassManager {
    fn default() -> Self {
        PassManager::new()
    }
}

/// Whole-module optimization: reference-counting insertion, then aggressive tree-shaking interleaved
/// with function inlining, run to a fixpoint.
///
/// Crucially, `RcInsertion` runs *before* inlining. Dream has deterministic, reference-counted
/// destruction, so a local reference's lifetime must end at the point its owning function returns —
/// not at the caller's scope exit. Inserting RC first bakes each callee's scope-exit `Release`s into
/// its body, so inlining copies them to the return site (the continuation), preserving object
/// lifetimes exactly. Inlining a callee whose value it *returns* moves the transferred `+1` into the
/// call's destination via a plain copy, which is balanced because the callee already skipped
/// releasing the returned value.
///
/// After inlining, [`crate::driver`] runs the per-function [`PassManager`] (copy/const prop, folding,
/// algebraic, GVN, CFG simplification, DCE, and RC elision) to clean up the merged bodies.
pub fn optimize_module(mir: &mut Mir, interner: &TypeInterner) {
    const MAX_ROUNDS: usize = 8;
    crate::mir::prune_module(mir);
    let rc = RcInsertion;
    for f in &mut mir.functions {
        rc.run(f, interner);
    }
    let inliner = Inliner::default();
    for _ in 0..MAX_ROUNDS {
        let changed = inliner.run(mir, interner);
        // Drop callees left with no remaining call sites after inlining (plus their transitively
        // dead callees), then loop: the smaller module may expose more inlining.
        crate::mir::prune_module(mir);
        if !changed {
            break;
        }
    }
}
