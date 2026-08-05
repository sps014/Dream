//! Sync-function control flow: walk [`crate::mir::relooper::Shape`] into nested WASM
//! `block`/`loop`/`if`. Async poll functions keep the `$__pc`/`br_table` dispatch in
//! [`super::async_ops`] and never enter this path.
//!
//! When the shape walker cannot resolve a branch to a structured label (or the relooper
//! produces a multi-entry loop body), emission falls back to classic PC/`br_table` dispatch.

use super::*;
use crate::mir::relooper::Shape;
use crate::mir::BlockId;
use std::collections::BTreeMap;

/// One structured scope opened by a [`Shape::Loop`] (and optionally a join [`Shape::Multiple`]).
pub(super) struct ShapeScope {
    /// Loop-header blocks → continue label (branch restarts the `loop`).
    continues: BTreeMap<BlockId, String>,
    /// Exit / join blocks → break label (branch leaves the enclosing `block`).
    breaks: BTreeMap<BlockId, String>,
}

impl Emitter<'_> {
    /// Reloop the CFG and emit nested structured control flow (sync functions only).
    pub(super) fn emit_shaped_body(&mut self) {
        let shape = crate::mir::relooper::reloop(self.func).unwrap_or_else(|| {
            crate::internal_error!("relooper failed on reducible Dream CFG")
        });
        if shape_needs_pc_dispatch(&shape) {
            self.emit_pc_dispatch();
            return;
        }
        let checkpoint = self.out.len();
        let line_checkpoint = self.current_line;
        self.shape_scopes.clear();
        self.shape_label_id = 0;
        if self.emit_shape(&shape, None).is_err() {
            self.out.truncate(checkpoint);
            self.current_line = line_checkpoint;
            self.shape_scopes.clear();
            self.emit_pc_dispatch();
            return;
        }
        if self.wasm_returns_value() {
            self.line("  (unreachable)");
        }
    }

    /// Classic labeled-block dispatch (same scaffold async poll uses). Used when nested shape
    /// emit cannot label every edge.
    pub(super) fn emit_pc_dispatch(&mut self) {
        let n = self.func.blocks.len();
        self.line(&format!("  ;; entry = bb{} (pc dispatch fallback)", self.func.entry.0));
        self.line(&format!("  (i32.const {})", self.func.entry.0));
        self.line("  (local.set $__pc)");
        self.line("  (block $host_exit");
        self.line("   (loop $__loop");
        for i in (0..n).rev() {
            self.line(&format!("    (block $bb{}", i));
        }
        let labels: String = (0..n).map(|i| format!("$bb{} ", i)).collect();
        let default = format!("$bb{}", n.saturating_sub(1));
        self.line(&format!(
            "     (br_table {}{} (local.get $__pc))",
            labels, default
        ));
        for i in 0..n {
            self.line(&format!("    ) ;; bb{} body", i));
            let block = self.func.block(crate::mir::BlockId(i as u32));
            for stmt in &block.stmts {
                self.emit_stmt(stmt);
            }
            self.emit_terminator(&block.terminator);
        }
        self.line("   )");
        self.line("  )");
        if self.wasm_returns_value() {
            self.line("  (unreachable)");
        }
    }

    fn emit_shape(&mut self, shape: &Shape, fallthrough: Option<BlockId>) -> Result<(), ()> {
        match shape {
            Shape::Simple { block, next } => {
                self.emit_shape_simple(*block, next.as_deref(), fallthrough)
            }
            Shape::Loop { inner, next } => {
                self.emit_shape_loop(inner, next.as_deref(), fallthrough)
            }
            Shape::Multiple { handled, next } => {
                self.emit_shape_multiple(handled, next.as_deref(), fallthrough)
            }
        }
    }

    fn emit_shape_simple(
        &mut self,
        block: BlockId,
        next: Option<&Shape>,
        fallthrough: Option<BlockId>,
    ) -> Result<(), ()> {
        let b = self.func.block(block);
        for stmt in &b.stmts {
            self.emit_stmt(stmt);
        }
        let term = b.terminator.clone();
        match term {
            Terminator::Goto(t) => {
                let next_entry = next.and_then(shape_entry);
                if next_entry == Some(t) {
                    if let Some(n) = next {
                        self.emit_shape(n, fallthrough)?;
                    }
                } else {
                    self.shape_branch_to(t, fallthrough.or(next_entry))?;
                }
            }
            Terminator::If {
                cond,
                then_blk,
                else_blk,
            } => {
                self.emit_shape_if(cond, then_blk, else_blk, next, fallthrough)?;
            }
            Terminator::Switch {
                value,
                targets,
                default,
            } => {
                self.emit_shape_switch(value, &targets, default, next, fallthrough)?;
            }
            Terminator::Return(_)
            | Terminator::TailCall { .. }
            | Terminator::Unreachable
            | Terminator::AsyncComplete(_)
            | Terminator::Await { .. } => {
                self.emit_terminator(&term);
            }
        }
        Ok(())
    }

    fn emit_shape_if(
        &mut self,
        cond: Operand,
        then_blk: BlockId,
        else_blk: BlockId,
        next: Option<&Shape>,
        fallthrough: Option<BlockId>,
    ) -> Result<(), ()> {
        match next {
            Some(Shape::Multiple { handled, next: join }) => {
                let join_ft = join.as_deref().and_then(shape_entry).or(fallthrough);
                let then_shape = find_arm(handled, then_blk);
                let else_shape = find_arm(handled, else_blk);
                self.emit_operand(&cond);
                self.line("     (if (then");
                if let Some(s) = then_shape {
                    self.emit_shape(s, join_ft)?;
                } else {
                    self.shape_branch_to(then_blk, join_ft)?;
                }
                self.line("     ) (else");
                if let Some(s) = else_shape {
                    self.emit_shape(s, join_ft)?;
                } else {
                    self.shape_branch_to(else_blk, join_ft)?;
                }
                self.line("     ))");
                if let Some(j) = join.as_deref() {
                    self.emit_shape(j, fallthrough)?;
                }
            }
            Some(other) => {
                let entry = shape_entry(other);
                self.emit_operand(&cond);
                self.line("     (if (then");
                if entry == Some(then_blk) {
                    self.emit_shape(other, fallthrough)?;
                } else {
                    self.shape_branch_to(then_blk, fallthrough)?;
                }
                self.line("     ) (else");
                if entry == Some(else_blk) {
                    self.emit_shape(other, fallthrough)?;
                } else {
                    self.shape_branch_to(else_blk, fallthrough)?;
                }
                self.line("     ))");
            }
            None => {
                self.emit_operand(&cond);
                self.line("     (if (then");
                self.shape_branch_to(then_blk, fallthrough)?;
                self.line("     ) (else");
                self.shape_branch_to(else_blk, fallthrough)?;
                self.line("     ))");
            }
        }
        Ok(())
    }

    fn emit_shape_switch(
        &mut self,
        value: Operand,
        targets: &[(i64, BlockId)],
        default: BlockId,
        next: Option<&Shape>,
        fallthrough: Option<BlockId>,
    ) -> Result<(), ()> {
        match next {
            Some(Shape::Multiple {
                handled,
                next: join,
            }) => {
                let join_ft = join.as_deref().and_then(shape_entry).or(fallthrough);
                let id = self.fresh_shape_label();
                let skip_lbl = format!("__sw{}", id);
                self.line(&format!("     (block ${}", skip_lbl));
                for (k, b) in targets {
                    self.emit_operand(&value);
                    self.line(&format!("     (i32.const {})", k));
                    self.line("     (i32.eq)");
                    self.line("     (if (then");
                    if let Some(s) = find_arm(handled, *b) {
                        self.emit_shape(s, join_ft)?;
                    } else {
                        self.shape_branch_to(*b, join_ft)?;
                    }
                    self.line(&format!("     (br ${})", skip_lbl));
                    self.line("     ))");
                }
                if let Some(s) = find_arm(handled, default) {
                    self.emit_shape(s, join_ft)?;
                } else {
                    self.shape_branch_to(default, join_ft)?;
                }
                self.line("     )");
                if let Some(j) = join.as_deref() {
                    self.emit_shape(j, fallthrough)?;
                }
            }
            other => {
                let join_ft = fallthrough;
                let id = self.fresh_shape_label();
                let skip_lbl = format!("__sw{}", id);
                self.line(&format!("     (block ${}", skip_lbl));
                for (k, b) in targets {
                    self.emit_operand(&value);
                    self.line(&format!("     (i32.const {})", k));
                    self.line("     (i32.eq)");
                    self.line("     (if (then");
                    if other.and_then(shape_entry) == Some(*b) {
                        if let Some(s) = other {
                            self.emit_shape(s, join_ft)?;
                        }
                    } else {
                        self.shape_branch_to(*b, join_ft)?;
                    }
                    self.line(&format!("     (br ${})", skip_lbl));
                    self.line("     ))");
                }
                if other.and_then(shape_entry) == Some(default) {
                    if let Some(s) = other {
                        self.emit_shape(s, join_ft)?;
                    }
                } else {
                    self.shape_branch_to(default, join_ft)?;
                }
                self.line("     )");
            }
        }
        Ok(())
    }

    fn emit_shape_loop(
        &mut self,
        inner: &Shape,
        next: Option<&Shape>,
        fallthrough: Option<BlockId>,
    ) -> Result<(), ()> {
        let id = self.fresh_shape_label();
        let cont_lbl = format!("__cnt{}", id);
        let header = shape_entry(inner).ok_or(())?;

        let mut continues = BTreeMap::new();
        continues.insert(header, cont_lbl.clone());
        if let Shape::Multiple { handled, .. } = inner {
            for arm in handled {
                if let Some(e) = shape_entry(arm) {
                    continues.insert(e, cont_lbl.clone());
                }
            }
        }

        let exits = next.map(shape_exit_list).unwrap_or_default();
        let mut breaks = BTreeMap::new();
        let mut break_lbls: Vec<String> = Vec::new();
        for (i, (entry, _)) in exits.iter().enumerate() {
            let lbl = format!("__brk{}_{}", id, i);
            breaks.insert(*entry, lbl.clone());
            break_lbls.push(lbl);
        }
        if break_lbls.is_empty() {
            break_lbls.push(format!("__brk{}", id));
        }

        self.shape_scopes.push(ShapeScope { continues, breaks });
        for lbl in break_lbls.iter().rev() {
            self.line(&format!("     (block ${}", lbl));
        }
        self.line(&format!("      (loop ${}", cont_lbl));
        self.emit_shape(inner, None)?;
        self.line(&format!("      (br ${})", cont_lbl));
        self.line("      )");

        if exits.is_empty() {
            self.line("     )");
        } else {
            for (_entry, arm) in &exits {
                self.line("     )");
                self.emit_shape(arm, fallthrough)?;
            }
        }
        self.shape_scopes.pop();
        Ok(())
    }

    fn emit_shape_multiple(
        &mut self,
        handled: &[Shape],
        next: Option<&Shape>,
        fallthrough: Option<BlockId>,
    ) -> Result<(), ()> {
        let id = self.fresh_shape_label();
        let join_lbl = format!("__join{}", id);
        let join_ft = next.and_then(shape_entry).or(fallthrough);

        let mut breaks = BTreeMap::new();
        if let Some(e) = join_ft {
            breaks.insert(e, join_lbl.clone());
        }
        self.shape_scopes.push(ShapeScope {
            continues: BTreeMap::new(),
            breaks,
        });
        self.line(&format!("     (block ${}", join_lbl));
        for (i, arm) in handled.iter().enumerate() {
            self.emit_shape(arm, join_ft)?;
            if i + 1 < handled.len() {
                self.line(&format!("     (br ${})", join_lbl));
            }
        }
        self.line("     )");
        self.shape_scopes.pop();

        if let Some(n) = next {
            self.emit_shape(n, fallthrough)?;
        }
        Ok(())
    }

    /// `br` to a structured label for `target`, or no-op when `target == fallthrough`.
    /// Returns `Err` when no label exists — caller falls back to PC dispatch.
    fn shape_branch_to(
        &mut self,
        target: BlockId,
        fallthrough: Option<BlockId>,
    ) -> Result<(), ()> {
        if Some(target) == fallthrough {
            return Ok(());
        }
        for scope in self.shape_scopes.iter().rev() {
            if let Some(lbl) = scope.continues.get(&target) {
                self.line(&format!("     (br ${})", lbl));
                return Ok(());
            }
            if let Some(lbl) = scope.breaks.get(&target) {
                self.line(&format!("     (br ${})", lbl));
                return Ok(());
            }
        }
        Err(())
    }

    fn fresh_shape_label(&mut self) -> u32 {
        let id = self.shape_label_id;
        self.shape_label_id += 1;
        id
    }
}

fn shape_entry(shape: &Shape) -> Option<BlockId> {
    match shape {
        Shape::Simple { block, .. } => Some(*block),
        Shape::Loop { inner, .. } => shape_entry(inner),
        Shape::Multiple { handled, .. } => handled.first().and_then(shape_entry),
    }
}

/// Multi-entry loop bodies always need PC dispatch (label-variable loop tops not implemented).
fn shape_needs_pc_dispatch(shape: &Shape) -> bool {
    match shape {
        Shape::Loop { inner, next } => {
            let multi_entry = matches!(
                inner.as_ref(),
                Shape::Multiple { handled, .. } if handled.len() > 1
            );
            multi_entry
                || shape_needs_pc_dispatch(inner)
                || next.as_ref().is_some_and(|n| shape_needs_pc_dispatch(n))
        }
        Shape::Simple { next, .. } => next.as_ref().is_some_and(|n| shape_needs_pc_dispatch(n)),
        Shape::Multiple { handled, next } => {
            handled.iter().any(shape_needs_pc_dispatch)
                || next.as_ref().is_some_and(|n| shape_needs_pc_dispatch(n))
        }
    }
}

fn shape_exit_list(shape: &Shape) -> Vec<(BlockId, &Shape)> {
    match shape {
        Shape::Multiple { handled, next } => {
            let mut out = Vec::new();
            for arm in handled {
                if let Some(e) = shape_entry(arm) {
                    out.push((e, arm));
                }
            }
            if let Some(n) = next.as_deref() {
                out.extend(shape_exit_list(n));
            }
            out
        }
        other => shape_entry(other)
            .map(|e| vec![(e, other)])
            .unwrap_or_default(),
    }
}

fn find_arm(handled: &[Shape], entry: BlockId) -> Option<&Shape> {
    handled.iter().find(|s| shape_entry(s) == Some(entry))
}
