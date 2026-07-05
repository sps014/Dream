//! Expression lowering to [`Operand`]/[`Rvalue`]/[`Place`], including the short-circuiting forms
//! (`&&`/`||`/`?:`/`??`) and `await`. Split out of `lower.rs`; these are methods on the parent's
//! private `Lowerer`.

use super::*;

impl Lowerer<'_> {
    /// Selects the integer constant width from the literal's static type: `long`/`ulong` lower to a
    /// 64-bit [`Const::Long`], everything else (`int`/`uint`/`byte`) to a 32-bit [`Const::Int`].
    fn int_const(&self, ty: TypeId, v: i64) -> Const {
        match self.interner.kind(self.interner.strip_nullable(ty)) {
            TyKind::Prim(PrimTy::Long | PrimTy::ULong) => Const::Long(v),
            _ => Const::Int(v),
        }
    }

    /// Selects the float constant width from the literal's static type: `float` lowers to a 32-bit
    /// [`Const::F32`], `double` (and anything else) to a 64-bit [`Const::Float`].
    fn float_const(&self, ty: TypeId, v: f64) -> Const {
        match self.interner.kind(self.interner.strip_nullable(ty)) {
            TyKind::Prim(PrimTy::Float) => Const::F32(v as f32),
            _ => Const::Float(v),
        }
    }

    /// Lowers an expression to an operand, materializing computation into a fresh temporary.
    pub(super) fn lower_operand(&mut self, e: &HExpr) -> Operand {
        match &e.kind {
            HExprKind::IntLit(v) => Operand::Const(self.int_const(e.ty, *v)),
            HExprKind::FloatLit(v) => Operand::Const(self.float_const(e.ty, *v)),
            HExprKind::BoolLit(v) => Operand::Const(Const::Bool(*v)),
            HExprKind::CharLit(v) => Operand::Const(Const::Char(*v)),
            HExprKind::StringLit(s) => Operand::Const(Const::Str(s.clone())),
            HExprKind::Null => Operand::Const(Const::Null),
            HExprKind::EnumValue(v) => Operand::Const(Const::Int(*v)),
            HExprKind::Var(Binding::Local(l)) => Operand::Copy(Place::Local(self.mir_local(*l))),
            HExprKind::Var(Binding::Global(g)) => {
                Operand::Copy(Place::Global(super::super::Global(g.0)))
            }
            HExprKind::Binary { op, .. } if op.is_logical() => self.lower_short_circuit(e),
            HExprKind::Ternary { .. } => self.lower_ternary(e),
            HExprKind::Coalesce { .. } => self.lower_coalesce(e),
            // In a coroutine, `await` is a suspend point that splits the current block; elsewhere the
            // outer await is a no-op wrapper (the value is the inner future expression).
            HExprKind::Await(inner) => {
                if self.async_coroutine {
                    self.lower_await(e)
                } else {
                    self.lower_operand(inner)
                }
            }
            _ => {
                let rv = self.lower_rvalue(e);
                let temp = self.b.new_temp(e.ty);
                self.b.assign(Place::Local(temp), rv);
                Operand::Copy(Place::Local(temp))
            }
        }
    }

    /// Lowers an expression into an rvalue (the form usable on an assignment RHS).
    pub(super) fn lower_rvalue(&mut self, e: &HExpr) -> Rvalue {
        match &e.kind {
            HExprKind::Binary { op, lhs, rhs } if !op.is_logical() => {
                let l = self.lower_operand(lhs);
                let r = self.lower_operand(rhs);
                Rvalue::Binary(*op, l, r)
            }
            HExprKind::Unary { op, operand } => {
                let o = self.lower_operand(operand);
                Rvalue::Unary(*op, o)
            }
            HExprKind::Call { callee, args } => {
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::Call {
                    callee: self.lower_callee(callee),
                    args: lowered,
                }
            }
            HExprKind::MethodCall {
                receiver,
                callee,
                args,
            } => {
                let mut lowered = vec![self.lower_operand(receiver)];
                lowered.extend(args.iter().map(|a| self.lower_operand(a)));
                Rvalue::Call {
                    callee: self.lower_callee(callee),
                    args: lowered,
                }
            }
            HExprKind::IndirectCall { target, args } => {
                let t = self.lower_operand(target);
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::IndirectCall {
                    target: t,
                    args: lowered,
                }
            }
            HExprKind::InterfaceCall {
                receiver,
                iface_id,
                method_slot,
                sig,
                args,
            } => {
                let recv = self.lower_operand(receiver);
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::InterfaceCall {
                    receiver: recv,
                    iface_id: *iface_id,
                    method_slot: *method_slot,
                    sig: *sig,
                    args: lowered,
                    ret: e.ty,
                }
            }
            // A function name used as a value becomes its function-table index.
            HExprKind::Var(Binding::Func(callee)) => Rvalue::FuncRef(self.lower_callee(callee)),
            HExprKind::New {
                def, ctor, args, ..
            } => {
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::New {
                    def: *def,
                    ty: e.ty,
                    ctor: *ctor,
                    args: lowered,
                }
            }
            HExprKind::UnionNew { def, variant, args } => {
                let lowered = args.iter().map(|a| self.lower_operand(a)).collect();
                Rvalue::UnionNew {
                    def: *def,
                    ty: e.ty,
                    variant: *variant,
                    args: lowered,
                }
            }
            HExprKind::Field { obj, field } => {
                let base = self.operand_into_local(obj);
                Rvalue::Use(Operand::Copy(Place::Field {
                    base,
                    field: *field,
                }))
            }
            HExprKind::Index { array, index } => {
                let base = self.operand_into_local(array);
                let idx = self.lower_operand(index);
                Rvalue::Use(Operand::Copy(Place::Index {
                    base,
                    index: Box::new(idx),
                }))
            }
            HExprKind::Discriminant(v) => Rvalue::Discriminant(self.lower_operand(v)),
            HExprKind::IsType { value, target } => {
                Rvalue::IsType(self.lower_operand(value), *target)
            }
            HExprKind::UnionField {
                base,
                union_ty,
                variant,
                field,
            } => Rvalue::UnionField {
                base: self.lower_operand(base),
                ty: *union_ty,
                variant: *variant,
                field: *field,
            },
            HExprKind::ArrayLen(a) => Rvalue::ArrayLen(self.lower_operand(a)),
            HExprKind::StrLen(a) => Rvalue::StrLen(self.lower_operand(a)),
            HExprKind::CharAt(s, i) => Rvalue::CharAt(self.lower_operand(s), self.lower_operand(i)),
            HExprKind::ArrayNew { elem_ty, len } => Rvalue::ArrayNew {
                elem_ty: *elem_ty,
                len: self.lower_operand(len),
            },
            HExprKind::ToBytes(v) => Rvalue::ToBytes {
                value: self.lower_operand(v),
                ty: v.ty,
            },
            HExprKind::FromBytes(bytes) => Rvalue::FromBytes {
                bytes: self.lower_operand(bytes),
                ty: e.ty,
            },
            HExprKind::HashCode(e) => Rvalue::HashCode(self.lower_operand(e)),
            HExprKind::ToString(e) => Rvalue::ToString(self.lower_operand(e)),
            HExprKind::Concat(a, b) => Rvalue::Concat(self.lower_operand(a), self.lower_operand(b)),
            HExprKind::EnumName { value, arms } => Rvalue::EnumName {
                value: self.lower_operand(value),
                arms: arms.clone(),
            },
            HExprKind::ArrayLit { elem_ty, elems } => {
                let lowered = elems.iter().map(|e| self.lower_operand(e)).collect();
                Rvalue::ArrayLit {
                    elem_ty: *elem_ty,
                    elems: lowered,
                }
            }
            HExprKind::JsCall {
                callee,
                target,
                method,
                args,
            } => {
                let target = self.lower_operand(target);
                let method = method.as_ref().map(|m| self.lower_operand(m));
                let args = args.iter().map(|a| (self.lower_operand(a), a.ty)).collect();
                Rvalue::JsCall {
                    callee: self.lower_callee(callee),
                    target,
                    method,
                    args,
                }
            }
            HExprKind::Cast(inner) => {
                let from = inner.ty;
                Rvalue::Cast(self.lower_operand(inner), from, e.ty)
            }
            // `await` as an rvalue routes through `lower_operand`, which suspends in a coroutine
            // (materializing the awaited result into a temp) or unwraps to the inner future otherwise.
            HExprKind::Await(_) => Rvalue::Use(self.lower_operand(e)),
            // Already-operand-shaped or short-circuiting forms: go through `lower_operand`.
            _ => Rvalue::Use(self.lower_operand(e)),
        }
    }

    fn operand_into_local(&mut self, e: &HExpr) -> Local {
        match self.lower_operand(e) {
            Operand::Copy(Place::Local(l)) => l,
            other => {
                let t = self.b.new_temp(e.ty);
                self.b.assign(Place::Local(t), Rvalue::Use(other));
                t
            }
        }
    }

    /// Lowers `await e` (in a coroutine) to a suspend point: the future `e` is evaluated in the
    /// current block, which ends with a [`Terminator::Await`] parking the task; lowering continues in
    /// a fresh `resume` block where the settled result is bound to `dest`. Returns the `dest` read, so
    /// callers see `await e` as an ordinary value — hence awaits compose in any sub-expression.
    fn lower_await(&mut self, await_expr: &HExpr) -> Operand {
        let HExprKind::Await(inner) = &await_expr.kind else {
            unreachable!("lower_await on non-await expression");
        };
        let future = self.lower_operand(inner);
        let dest = self.b.new_temp(await_expr.ty);
        let resume = self.b.new_block();
        self.b.terminate(Terminator::Await {
            future,
            dest: Some(dest),
            resume,
        });
        self.b.switch_to(resume);
        Operand::Copy(Place::Local(dest))
    }

    pub(super) fn lower_callee(&self, callee: &crate::hir::Callee) -> super::super::Callee {
        super::super::Callee {
            def: callee.def,
            args: callee.instance.clone(),
            ret: callee.ret,
        }
    }

    /// `a && b` / `a || b`: evaluate `b` only on the deciding branch, joining into one bool temp.
    fn lower_short_circuit(&mut self, e: &HExpr) -> Operand {
        let (op, lhs, rhs) = match &e.kind {
            HExprKind::Binary { op, lhs, rhs } => (*op, lhs, rhs),
            _ => unreachable!("lower_short_circuit on non-binary"),
        };
        let result = self.b.new_temp(e.ty);
        let l = self.lower_operand(lhs);

        let rhs_blk = self.b.new_block();
        let short_blk = self.b.new_block();
        let join = self.b.new_block();

        // `&&`: if lhs then evaluate rhs else result=false. `||`: if lhs then result=true else rhs.
        let (then_blk, else_blk) = if op == super::super::BinOp::And {
            (rhs_blk, short_blk)
        } else {
            (short_blk, rhs_blk)
        };
        self.b.terminate(Terminator::If {
            cond: l,
            then_blk,
            else_blk,
        });

        self.b.switch_to(short_blk);
        let short_val = op == super::super::BinOp::Or;
        self.b.assign(
            Place::Local(result),
            Rvalue::Use(Operand::Const(Const::Bool(short_val))),
        );
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(rhs_blk);
        let r = self.lower_operand(rhs);
        self.b.assign(Place::Local(result), Rvalue::Use(r));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    fn lower_ternary(&mut self, e: &HExpr) -> Operand {
        let (cond, then_e, else_e) = match &e.kind {
            HExprKind::Ternary {
                cond,
                then_expr,
                else_expr,
            } => (cond, then_expr, else_expr),
            _ => unreachable!(),
        };
        let result = self.b.new_temp(e.ty);
        let c = self.lower_operand(cond);
        let then_blk = self.b.new_block();
        let else_blk = self.b.new_block();
        let join = self.b.new_block();
        self.b.terminate(Terminator::If {
            cond: c,
            then_blk,
            else_blk,
        });

        self.b.switch_to(then_blk);
        let tv = self.lower_operand(then_e);
        self.b.assign(Place::Local(result), Rvalue::Use(tv));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(else_blk);
        let ev = self.lower_operand(else_e);
        self.b.assign(Place::Local(result), Rvalue::Use(ev));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    fn lower_coalesce(&mut self, e: &HExpr) -> Operand {
        // `lhs ?? rhs`: result = lhs unless lhs is null, then rhs.
        let (lhs, rhs) = match &e.kind {
            HExprKind::Coalesce { lhs, rhs } => (lhs, rhs),
            _ => unreachable!(),
        };
        let result = self.b.new_temp(e.ty);
        let l = self.operand_into_local(lhs);

        let is_null = self.b.new_temp(self.interner.bool());
        self.b.assign(
            Place::Local(is_null),
            Rvalue::Binary(
                super::super::BinOp::Eq,
                Operand::Copy(Place::Local(l)),
                Operand::Const(Const::Null),
            ),
        );
        let rhs_blk = self.b.new_block();
        let lhs_blk = self.b.new_block();
        let join = self.b.new_block();
        self.b.terminate(Terminator::If {
            cond: Operand::Copy(Place::Local(is_null)),
            then_blk: rhs_blk,
            else_blk: lhs_blk,
        });

        self.b.switch_to(lhs_blk);
        self.b.assign(
            Place::Local(result),
            Rvalue::Use(Operand::Copy(Place::Local(l))),
        );
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(rhs_blk);
        let rv = self.lower_operand(rhs);
        self.b.assign(Place::Local(result), Rvalue::Use(rv));
        self.b.terminate(Terminator::Goto(join));

        self.b.switch_to(join);
        Operand::Copy(Place::Local(result))
    }

    pub(super) fn lower_place(&mut self, place: &HPlace) -> Place {
        match place {
            HPlace::Local(l) => Place::Local(self.mir_local(*l)),
            HPlace::Global(g) => Place::Global(super::super::Global(g.0)),
            HPlace::Field { obj, field } => {
                let base = self.operand_into_local(obj);
                Place::Field {
                    base,
                    field: *field,
                }
            }
            HPlace::Index { array, index } => {
                let base = self.operand_into_local(array);
                let idx = self.lower_operand(index);
                Place::Index {
                    base,
                    index: Box::new(idx),
                }
            }
        }
    }
}
