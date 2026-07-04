//! End-to-end test of the backend pipeline: a hand-built typed HIR program is lowered to MIR, run
//! through the full optimization pass pipeline, and emitted to WAT. This is the exact chain the
//! driver runs, so it both proves the pipeline composes and pins its determinism contract
//! (byte-identical output).

use dream::hir::{
    BinOp, Binding, HExpr, HExprKind, HFunction, HParam, HPlace, HStmt, Hir, LocalId,
};
use dream::mir::emit::emit_program;
use dream::mir::lower::lower_program;
use dream::mir::passes::{
    ConstFold, CopyConstProp, Dce, PassManager, RcElision, RcInsertion, SimplifyCfg,
};
use dream::types::{DefKind, TypeCtx};

/// Builds, lowers, optimizes, and emits the following program, returning the WAT text:
///
/// ```text
/// fun sum_to(n: int): int {
///     let i: int = 0;
///     let acc: int = 0;
///     while (i < n) { acc = acc + i; i = i + 1; }
///     return acc;
/// }
/// ```
///
/// A fresh `TypeCtx` is used each call so the result depends only on the pipeline, not on shared
/// interner state — which is what makes the determinism assertion meaningful.
fn compile_sum_to() -> String {
    let mut ctx = TypeCtx::new();
    let def = ctx.register(DefKind::Function, "sum_to", vec![]);
    let int = ctx.interner.int();
    let boolean = ctx.interner.bool();

    let n = LocalId(0);
    let i = LocalId(1);
    let acc = LocalId(2);

    let var = |local: LocalId| HExpr::new(int, HExprKind::Var(Binding::Local(local)));

    let func = HFunction {
        def,
        name: "sum_to".into(),
        instance: vec![],
        params: vec![HParam {
            local: n,
            name: "n".into(),
            ty: int,
        }],
        ret: int,
        locals: vec![
            dream::hir::HLocal {
                id: i,
                name: "i".into(),
                ty: int,
            },
            dream::hir::HLocal {
                id: acc,
                name: "acc".into(),
                ty: int,
            },
        ],
        is_async: false,
        file: None,
        body: vec![
            HStmt::Let {
                local: i,
                ty: int,
                value: HExpr::new(int, HExprKind::IntLit(0)),
            },
            HStmt::Let {
                local: acc,
                ty: int,
                value: HExpr::new(int, HExprKind::IntLit(0)),
            },
            HStmt::While {
                cond: HExpr::new(
                    boolean,
                    HExprKind::Binary {
                        op: BinOp::Lt,
                        lhs: Box::new(var(i)),
                        rhs: Box::new(var(n)),
                    },
                ),
                body: vec![
                    HStmt::Assign {
                        place: HPlace::Local(acc),
                        value: HExpr::new(
                            int,
                            HExprKind::Binary {
                                op: BinOp::Add,
                                lhs: Box::new(var(acc)),
                                rhs: Box::new(var(i)),
                            },
                        ),
                    },
                    HStmt::Assign {
                        place: HPlace::Local(i),
                        value: HExpr::new(
                            int,
                            HExprKind::Binary {
                                op: BinOp::Add,
                                lhs: Box::new(var(i)),
                                rhs: Box::new(HExpr::new(int, HExprKind::IntLit(1))),
                            },
                        ),
                    },
                ],
                label: None,
            },
            HStmt::Return(Some(var(acc))),
        ],
    };

    let hir = Hir {
        functions: vec![func],
        globals: vec![],
        instances: vec![],
        ..Default::default()
    };

    let mut mir = lower_program(&hir, &ctx.interner);

    // Mirror the intended production pipeline, exercising every shipped pass (RC insertion/elision
    // are no-ops here since the function is reference-free, but must still compose cleanly).
    let mut pm = PassManager::new();
    pm.add(CopyConstProp);
    pm.add(ConstFold);
    pm.add(SimplifyCfg);
    pm.add(Dce);
    pm.add(RcInsertion);
    pm.add(RcElision);
    for f in &mut mir.functions {
        pm.run(f, &ctx.interner);
    }

    emit_program(&mir, &ctx.interner)
}

#[test]
fn hir_to_wat_pipeline_emits_expected_shape() {
    let wat = compile_sum_to();

    assert!(
        wat.contains("(func $sum_to"),
        "missing function header:\n{}",
        wat
    );
    assert!(
        wat.contains("(param $0 i32)"),
        "missing typed parameter:\n{}",
        wat
    );
    assert!(
        wat.contains("(result i32)"),
        "missing typed result:\n{}",
        wat
    );
    // The loop body's two additions survive optimization (they are live).
    assert!(wat.contains("i32.add"), "missing arithmetic:\n{}", wat);
    // The loop comparison lowers to a signed less-than.
    assert!(
        wat.contains("i32.lt_s"),
        "missing loop comparison:\n{}",
        wat
    );
    // A multi-block CFG is emitted via the block-dispatch loop.
    assert!(wat.contains("br_table"), "missing CFG dispatch:\n{}", wat);
}

#[test]
fn hir_to_wat_pipeline_is_deterministic() {
    let first = compile_sum_to();
    let second = compile_sum_to();
    assert_eq!(
        first, second,
        "the new backend pipeline must be byte-for-byte deterministic"
    );
}
