//! `lock (target) { body }` — mutual exclusion on an `@shared class` instance (see
//! `docs/language` concurrency notes and the shared-memory-WebWorkers plan). Type-checks `target`,
//! then lowers to `HStmt::Lock`; the acquire/release-on-every-exit-path lowering itself happens in
//! `src/mir/lower/control_flow.rs::lower_lock`, since MIR is where "every exit path" (fallthrough,
//! `return`, `break`, `continue`) is a concrete set of block edges to instrument.

use super::*;
use dream_syntax::nodes::StatementNode;

impl<'a> Analyzer<'a> {
    pub(in crate::analyzer) fn analyze_lock(
        &mut self,
        target: &ExpressionNode<'a>,
        body: &[StatementNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        has_parent_while: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let target_type = self
            .analyze_expression(target, parent_function, symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let target_hir = self.hir_take();
        if !target_type.is_unknown() {
            let tid = self.type_ctx.lower(&target_type);
            if !self.type_ctx.interner.is_shared_type(tid) {
                diagnostics.report_error(
                    format!(
                        "'lock' target must be an '@shared class' instance, got '{}'",
                        target_type.get_type()
                    ),
                    target.position(),
                );
            }
        }
        let body_scope = self.branch_scope(symbol_table);
        self.hir_open_block();
        self.analyze_body(
            body,
            parent_function,
            Some(&body_scope),
            has_parent_while,
            diagnostics,
        )?;
        let body_hir = self.hir_close_block();
        self.hir_lock(target_hir, body_hir);
        Ok(())
    }
}
