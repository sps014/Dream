//! Call-expression analysis, grouped by call shape:
//! - [`free_functions`]: free-function calls and overload selection entry points.
//! - [`member_calls`]: instance/static/namespaced method calls, plus the indexer/enumerator "hook"
//!   resolution (`get`/`set`/`iterator`/`next`) used to desugar `obj[i]`, `obj[i] = v`, and
//!   `for..in`. `resolve_hook_or_diagnose` there is the shared entry point those desugaring sites
//!   call; the `HookResolution` outcome is an implementation detail kept private to that module.
//! - [`overload_resolution`]: scoring/ranking of candidate overloads.
//! - [`constructor`]: constructor-call analysis.

pub(crate) mod constructor;
pub(crate) mod free_functions;
pub(crate) mod member_calls;
pub(crate) mod overload_resolution;

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::hir::HExpr;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::{ExpressionNode, FunctionNode};
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Analyzes each explicit call argument in source order, returning the argument type strings
    /// alongside the HIR emitted for each. Shared by the static, instance, and interface call paths,
    /// which all analyze their arguments identically before resolving the callee. (The free-function
    /// path publishes per-parameter expected types while analyzing and so keeps its own loop.)
    pub(super) fn analyze_call_arguments(
        &mut self,
        params: &[ExpressionNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(Vec<String>, Vec<Option<HExpr>>), SemanticError> {
        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        for param in params.iter() {
            let t = self.analyze_expression(param, parent_function, symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
            arg_types.push(t.get_type());
        }
        Ok((arg_types, arg_hirs))
    }
}
