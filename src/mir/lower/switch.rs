//! `switch`/`match` lowering, dispatched by [`Lowerer::lower_switch`] to one of three strategies
//! depending on the scrutinee/pattern shape: a union `match` dispatches on the discriminant and
//! binds each arm's payload ([`Lowerer::lower_variant_switch`]); a `string` switch needs content
//! equality, so it lowers to a linear compare chain ([`Lowerer::lower_string_switch`]); everything
//! else (int/enum/bool) dispatches on the scrutinee value via a `br_table`
//! ([`Lowerer::lower_const_switch`]). Split out of `lower.rs`; these are methods on the parent's
//! private `Lowerer`.

use super::*;

impl Lowerer<'_> {
    pub(super) fn lower_switch(
        &mut self,
        scrutinee: &HExpr,
        arms: &[crate::hir::HArm],
        default: &[HStmt],
    ) {
        // A union `match` dispatches on the value's discriminant and binds each arm's payload; a
        // string `switch` needs content equality (no `br_table`); an int/enum/bool `switch`
        // dispatches on the scrutinee value itself via a `br_table`.
        if arms
            .iter()
            .any(|a| matches!(a.pattern, crate::hir::HPattern::Variant { .. }))
        {
            self.lower_variant_switch(scrutinee, arms, default);
        } else if matches!(
            self.interner
                .kind(self.interner.strip_nullable(scrutinee.ty)),
            TyKind::Prim(PrimTy::String)
        ) {
            self.lower_string_switch(scrutinee, arms, default);
        } else {
            self.lower_const_switch(scrutinee, arms, default);
        }
    }

    /// Lowers a `switch` on a string: a linear chain of `subject == label` content comparisons (the
    /// backend routes string `==` through `$string_eq`), each branching to its arm or the next check.
    fn lower_string_switch(
        &mut self,
        scrutinee: &HExpr,
        arms: &[crate::hir::HArm],
        default: &[HStmt],
    ) {
        let subject = self.lower_operand(scrutinee);
        let join = self.b.new_block();
        for arm in arms {
            let crate::hir::HPattern::Const(label) = &arm.pattern else {
                continue;
            };
            let label_op = self.lower_operand(label);
            let cond = self.b.new_temp(self.interner.bool());
            self.b.assign(
                Place::Local(cond),
                Rvalue::Binary(super::super::BinOp::Eq, subject.clone(), label_op),
            );
            let body_blk = self.b.new_block();
            let next_blk = self.b.new_block();
            self.b.terminate(Terminator::If {
                cond: Operand::Copy(Place::Local(cond)),
                then_blk: body_blk,
                else_blk: next_blk,
            });
            self.b.switch_to(body_blk);
            self.lower_block(&arm.body);
            if !self.b.is_terminated() {
                self.b.terminate(Terminator::Goto(join));
            }
            self.b.switch_to(next_blk);
        }
        // Chain fell through every case: run the default, then join.
        self.lower_block(default);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }
        self.b.switch_to(join);
    }

    /// Lowers a `switch`/`match` whose arms are const/enum-valued: a `br_table` over the scrutinee.
    fn lower_const_switch(
        &mut self,
        scrutinee: &HExpr,
        arms: &[crate::hir::HArm],
        default: &[HStmt],
    ) {
        let value = self.lower_operand(scrutinee);
        let default_blk = self.b.new_block();
        let join = self.b.new_block();
        let mut targets: Vec<(i64, super::super::BlockId)> = Vec::new();

        for arm in arms {
            let blk = self.b.new_block();
            if let crate::hir::HPattern::Const(c) = &arm.pattern {
                if let Some(v) = const_int_value(c) {
                    targets.push((v, blk));
                }
            }
            let saved = self.b.current();
            self.b.switch_to(blk);
            self.lower_block(&arm.body);
            if !self.b.is_terminated() {
                self.b.terminate(Terminator::Goto(join));
            }
            self.b.switch_to(saved);
        }

        self.b.terminate(Terminator::Switch {
            value,
            targets,
            default: default_blk,
        });

        self.b.switch_to(default_blk);
        self.lower_block(default);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }

        self.b.switch_to(join);
    }

    /// Lowers a `match` on a discriminated union: read the value's discriminant, `br_table` on it,
    /// and in each variant arm bind the payload fields to their pattern locals before the body runs.
    fn lower_variant_switch(
        &mut self,
        scrutinee: &HExpr,
        arms: &[crate::hir::HArm],
        default: &[HStmt],
    ) {
        let union_ty = scrutinee.ty;
        let ptr = self.lower_operand(scrutinee);
        let disc = self.b.new_temp(self.interner.int());
        self.b
            .assign(Place::Local(disc), Rvalue::Discriminant(ptr.clone()));

        let default_blk = self.b.new_block();
        let join = self.b.new_block();
        let mut targets: Vec<(i64, super::super::BlockId)> = Vec::new();

        for arm in arms {
            let crate::hir::HPattern::Variant {
                variant, bindings, ..
            } = &arm.pattern
            else {
                continue;
            };
            let blk = self.b.new_block();
            targets.push((*variant as i64, blk));
            let saved = self.b.current();
            self.b.switch_to(blk);
            // Bind the active variant's payload fields; each is a borrow of the union's field (no
            // retain), so releasing the union later frees them exactly once.
            for (i, &binding) in bindings.iter().enumerate() {
                let local = self.mir_local(binding);
                self.b.assign(
                    Place::Local(local),
                    Rvalue::UnionField {
                        base: ptr.clone(),
                        ty: union_ty,
                        variant: *variant,
                        field: i,
                    },
                );
            }
            self.lower_block(&arm.body);
            if !self.b.is_terminated() {
                self.b.terminate(Terminator::Goto(join));
            }
            self.b.switch_to(saved);
        }

        self.b.terminate(Terminator::Switch {
            value: Operand::Copy(Place::Local(disc)),
            targets,
            default: default_blk,
        });

        self.b.switch_to(default_blk);
        self.lower_block(default);
        if !self.b.is_terminated() {
            self.b.terminate(Terminator::Goto(join));
        }

        self.b.switch_to(join);
    }
}
