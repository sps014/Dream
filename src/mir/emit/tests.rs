    use super::*;
    use crate::mir::build::FunctionBuilder;
    use crate::mir::{Place, Rvalue, Terminator};

    #[test]
    fn module_wraps_and_resolves_call_symbols() {
        use crate::mir::Callee;
        use crate::types::DefId;
        let i = TypeInterner::new();

        // fun callee(): int { return 0; }  (def 1)
        let mut cb = FunctionBuilder::new("callee", i.int());
        cb.set_def(DefId(1), vec![]);
        cb.terminate(Terminator::Return(Some(Operand::Const(Const::Int(0)))));
        let callee = cb.finish();

        // fun caller(): int { return callee(); }  (def 2, calls def 1)
        let mut rb = FunctionBuilder::new("caller", i.int());
        rb.set_def(DefId(2), vec![]);
        let t = rb.new_temp(i.int());
        rb.assign(
            Place::Local(t),
            Rvalue::Call {
                callee: Callee { def: DefId(1), args: vec![], ret: i.int() },
                args: vec![],
            },
        );
        rb.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));
        let caller = rb.finish();

        let mir = crate::mir::Mir { functions: vec![callee, caller], ..Default::default() };
        let wat = emit_module(&mir, &i, false);
        assert!(wat.starts_with("(module"), "should be wrapped in a module:\n{}", wat);
        assert!(wat.contains("(func $callee"), "callee header:\n{}", wat);
        // The call site resolves to the callee's symbol, not a bare def index.
        assert!(wat.contains("(call $callee)"), "call must resolve to the header symbol:\n{}", wat);
        assert!(wat.contains("(export \"caller\""), "non-instance funcs are exported:\n{}", wat);
    }

    #[test]
    fn instance_functions_get_distinct_symbols() {
        use crate::types::DefId;
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("id", i.int());
        b.set_def(DefId(7), vec![i.int()]);
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Int(0)))));
        let f = b.finish();
        let wat = emit_function(&f, &i);
        // The instance args are encoded into the symbol so monomorphizations stay distinct.
        assert!(wat.contains(&format!("(func $id__{}", i.int().0)), "instance symbol:\n{}", wat);
    }

    #[test]
    fn field_access_uses_layout_offsets_and_widths() {
        use crate::hir::{FieldLayout, LayoutTable, TypeLayout};
        use crate::types::DefId;
        let mut i = TypeInterner::new();
        let def = DefId(3);
        let dbl = i.prim(PrimTy::Double);
        let int = i.int();
        let sty = i.struct_ty(def, vec![]);

        let mut layouts = LayoutTable::default();
        layouts.insert(
            sty,
            TypeLayout {
                name: "S".into(),
                fields: vec![
                    FieldLayout { offset: 0, ty: int, name: "a".into() },
                    FieldLayout { offset: 8, ty: dbl, name: "b".into() },
                ],
                size: 16,
            },
        );

        // fun read(p: S): double { return p.<field 1>; }
        let mut b = FunctionBuilder::new("read", dbl);
        b.set_def(DefId(9), vec![]);
        let p = b.new_param(sty, Some("p".into()));
        let t = b.new_temp(dbl);
        b.assign(
            Place::Local(t),
            Rvalue::Use(Operand::Copy(Place::Field { base: p, field: 1 })),
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));

        let mir = crate::mir::Mir { functions: vec![b.finish()], layouts, ..Default::default() };
        let wat = emit_program(&mir, &i);
        assert!(wat.contains("(i32.const 8)"), "field 1 sits at byte offset 8:\n{}", wat);
        assert!(wat.contains("(f64.load)"), "a double field loads as f64:\n{}", wat);
    }

    #[test]
    fn new_allocates_and_initializes_fields() {
        use crate::hir::{FieldLayout, LayoutTable, TypeLayout};
        use crate::mir::Rvalue;
        use crate::types::DefId;
        let mut i = TypeInterner::new();
        let def = DefId(5);
        let int = i.int();
        let sty = i.struct_ty(def, vec![]);

        let mut layouts = LayoutTable::default();
        layouts.insert(
            sty,
            TypeLayout {
                name: "S".into(),
                fields: vec![
                    FieldLayout { offset: 0, ty: int, name: "a".into() },
                    FieldLayout { offset: 4, ty: int, name: "b".into() },
                ],
                size: 8,
            },
        );

        // fun make(): S { return S(); }  -- implicit zero-arg default constructor
        let mut b = FunctionBuilder::new("make", sty);
        b.set_def(DefId(9), vec![]);
        let t = b.new_temp(sty);
        b.assign(
            Place::Local(t),
            Rvalue::New { def, ty: sty, ctor: None, args: vec![] },
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(t)))));

        let mir = crate::mir::Mir { functions: vec![b.finish()], layouts, ..Default::default() };
        let wat = emit_program(&mir, &i);
        assert!(wat.contains("(i32.const 8)"), "allocates the struct's data size:\n{}", wat);
        assert!(wat.contains("(call $malloc)"), "constructs via malloc:\n{}", wat);
        assert!(wat.contains("(local.set $__obj)"), "captures the object pointer:\n{}", wat);
        assert!(wat.contains("(i32.store)"), "zero-initializes fields:\n{}", wat);
    }

    #[test]
    fn strings_get_data_segments_and_addresses() {
        use crate::types::DefId;
        let i = TypeInterner::new();
        let str_ty = i.string();
        let mut b = FunctionBuilder::new("hello", str_ty);
        b.set_def(DefId(1), vec![]);
        b.terminate(Terminator::Return(Some(Operand::Const(Const::Str("hi".into())))));

        let mir = crate::mir::Mir { functions: vec![b.finish()], ..Default::default() };
        let wat = emit_module(&mir, &i, false);
        // The runtime constants are interned first (`true`/`false`/`-` then the object-protocol
        // `null`/`<object>`/`[`/`]`/`, `), so the user's "hi" follows at block 1192 / data pointer
        // 1204. Each block carries a 4-byte length prefix and no NUL terminator.
        assert!(wat.contains("(i32.const 1204)"), "string data pointer:\n{}", wat);
        // Its data segment (at the block start) is the heap-object block: header `size=0`, `tag=5`,
        // `ref_count=1`, then the length prefix `2`, then the bytes 'h','i' (no NUL terminator).
        assert!(
            wat.contains(
                "(data (i32.const 1192) \"\\00\\00\\00\\00\\05\\00\\00\\00\\01\\00\\00\\00\\02\\00\\00\\00\\68\\69\")"
            ),
            "string data segment:\n{}",
            wat
        );
    }

    #[test]
    fn emit_module_assembles_to_valid_wasm() {
        use crate::hir::{FieldLayout, LayoutTable, TypeLayout};
        use crate::mir::{Callee, MirGlobal, Rvalue};
        use crate::types::DefId;
        let mut i = TypeInterner::new();
        let int = i.int();
        let def = DefId(4);
        let sty = i.struct_ty(def, vec![]);

        let mut layouts = LayoutTable::default();
        layouts.insert(
            sty,
            TypeLayout {
                name: "S".into(),
                fields: vec![FieldLayout { offset: 0, ty: int, name: "a".into() }],
                size: 4,
            },
        );

        // fun helper(): int { return 7; }  (def 1)
        let mut hb = FunctionBuilder::new("helper", int);
        hb.set_def(DefId(1), vec![]);
        hb.terminate(Terminator::Return(Some(Operand::Const(Const::Int(7)))));

        // fun run(): int {
        //   let o = S(helper());   ; allocation + call + field store
        //   g0 = o.x;              ; global write from a field read
        //   return o.x;
        // }
        let mut rb = FunctionBuilder::new("run", int);
        rb.set_def(DefId(2), vec![]);
        let call_t = rb.new_temp(int);
        rb.assign(
            Place::Local(call_t),
            Rvalue::Call { callee: Callee { def: DefId(1), args: vec![], ret: int }, args: vec![] },
        );
        let obj = rb.new_temp(sty);
        rb.assign(
            Place::Local(obj),
            Rvalue::New { def, ty: sty, ctor: None, args: vec![Operand::Copy(Place::Local(call_t))] },
        );
        rb.assign(
            Place::Global(crate::mir::Global(0)),
            Rvalue::Use(Operand::Copy(Place::Field { base: obj, field: 0 })),
        );
        rb.terminate(Terminator::Return(Some(Operand::Copy(Place::Field { base: obj, field: 0 }))));

        let mir = crate::mir::Mir {
            functions: vec![hb.finish(), rb.finish()],
            globals: vec![MirGlobal { id: crate::mir::Global(0), ty: int }],
            layouts,
            ..Default::default()
        };
        let wat = emit_module(&mir, &i, false);
        // The real gate: the emitted module must assemble to valid WebAssembly.
        wat::parse_str(&wat)
            .unwrap_or_else(|e| panic!("emitted module failed to assemble: {}\n{}", e, wat));
    }

    #[test]
    fn emits_arithmetic_function() {
        let i = TypeInterner::new();
        let mut b = FunctionBuilder::new("add", i.int());
        let a = b.new_param(i.int(), Some("a".into()));
        let c = b.new_param(i.int(), Some("b".into()));
        let sum = b.new_temp(i.int());
        b.assign(
            Place::Local(sum),
            Rvalue::Binary(BinOp::Add, Operand::Copy(Place::Local(a)), Operand::Copy(Place::Local(c))),
        );
        b.terminate(Terminator::Return(Some(Operand::Copy(Place::Local(sum)))));
        let func = b.finish();

        let wat = emit_function(&func, &i);
        assert!(wat.contains("(func $add"), "should emit a function header");
        assert!(wat.contains("i32.add"), "should emit the add instruction:\n{}", wat);
        assert!(wat.contains("(return)"));
        assert!(wat.contains("br_table"));
    }

    /// Every `{TAG_*}`/`{minus}` placeholder in the object + format runtime must be substituted; a
    /// stray brace would emit a literal `{` into the module (and fail to assemble). Guards the
    /// substitution table in [`to_string_runtime`].
    #[test]
    fn to_string_runtime_has_no_unsubstituted_placeholders() {
        let mut strings = IndexMap::new();
        strings.insert("true".to_string(), 0u32);
        strings.insert("false".to_string(), 8u32);
        strings.insert("-".to_string(), 16u32);
        let runtime = to_string_runtime(&strings);
        assert!(
            !runtime.contains('{') && !runtime.contains('}'),
            "object/format runtime still contains an unsubstituted placeholder:\n{}",
            runtime
        );
    }

    /// `--debug` must actually instrument the allocator under the MIR backend: with it on, `$malloc`
    /// bumps the live/total counters; with it off the hot path stays clean.
    #[test]
    fn debug_alloc_toggles_allocator_instrumentation() {
        assert!(runtime_prelude(true).contains("global.set $live_objects"));
        assert!(!runtime_prelude(false).contains("global.set $live_objects"));
    }
