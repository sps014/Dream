use bumpalo::Bump;
use std::fs;
use tracing::info;

use crate::diagnostics::{render, DiagnosticBag};
use crate::driver::abi::emit_wasm_and_abi;
use crate::driver::error::CompileError;
use crate::driver::json_derive::generate_json_derives;
use crate::driver::prelude::merge_prelude;
use crate::driver::source_loader::{parse_file_recursive, ProgramAccumulator};
use crate::semantics::analyzer::Analyzer;
use crate::syntax::nodes::ProgramNode;
use crate::syntax::syntax_tree::SyntaxTree;

pub enum Target {
    Wasm,
}

/// Orchestrates the compilation pipeline: source loading (delegated to `source_loader`/`prelude`),
/// semantic analysis, code generation, and artifact emission (delegated to `abi`). Diagnostic
/// rendering is delegated to the `diagnostics` module.
pub struct Compiler {
    target: Target,
    /// When `true`, codegen emits allocator instrumentation so the `Debug.live_objects()` /
    /// `Debug.total_allocations()` probes report real values. Off by default (release builds pay
    /// no per-allocation cost); enabled via the CLI `--debug` flag or [`Compiler::with_debug_alloc`].
    debug_alloc: bool,
}

impl Compiler {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            debug_alloc: false,
        }
    }

    /// Builder: enable allocator instrumentation for this compilation.
    pub fn with_debug_alloc(mut self, on: bool) -> Self {
        self.debug_alloc = on;
        self
    }

    pub fn compile(&self, main_file_path: &String, out_path: &String) -> Result<(), CompileError> {
        info!("starting parsing and multi-file resolution");
        let mut acc = ProgramAccumulator::default();

        let arena = Bump::new();
        let mut diagnostics = DiagnosticBag::new(None);

        parse_file_recursive(main_file_path, &mut acc, &arena, &mut diagnostics)?;

        // The standard collections (List<T>, Map<K, V>) are embedded in the compiler and merged
        // into every program as a prelude. They are generic templates, so they emit no code unless
        // the program actually instantiates them.
        merge_prelude(
            &arena,
            &mut acc.all_functions,
            &mut acc.all_structs,
            &mut acc.all_interfaces,
            &mut acc.all_enums,
            &mut acc.all_extends,
            &mut diagnostics,
            &mut acc.file_contents,
        )?;

        // Auto-derive `to_json`/`from_json` converters for every `@json` class (must run after
        // all classes are collected so `@json` field cross-references resolve). The prelude merge
        // above always contributes stdlib structs (List/Map/...), so an empty struct set here means
        // the derive is running before collection completed.
        debug_assert!(
            !acc.all_structs.is_empty(),
            "generate_json_derives must run after prelude merge / class collection"
        );
        generate_json_derives(
            &arena,
            &acc.all_structs,
            &acc.all_enums,
            &mut acc.all_extends,
            &mut diagnostics,
            &mut acc.file_contents,
        )?;

        if diagnostics.has_errors() {
            render(&diagnostics, &acc.file_contents);
            return Err(CompileError::Syntax);
        }

        let combined_program = ProgramNode::new(
            vec![],
            acc.all_structs,
            acc.all_interfaces,
            acc.all_functions,
            acc.all_enums,
            acc.all_extends,
            acc.all_globals,
        );
        let ast = SyntaxTree::new(combined_program);

        info!("finished parsing");
        info!("starting semantic analysis");

        let mut analyzer = Analyzer::new(&ast, &arena);
        // `analyze` reports each error into the bag and returns a typed failure once any error was
        // recorded, short-circuiting before code generation runs on a poisoned program.
        let symbol_info = match analyzer.analyze(&mut diagnostics) {
            Ok(info) => info,
            Err(_) => {
                render(&diagnostics, &acc.file_contents);
                return Err(CompileError::Semantic);
            }
        };

        info!("finished semantic analysis");
        info!("starting code generation");

        // Lower the analyzer-emitted HIR to MIR, optimize, and emit a self-contained module.
        // Destructuring moves the owned `hir` out and drops `symbol_info`'s borrowing references,
        // releasing the `&mut analyzer` borrow so the shared interner can be read (the HIR references
        // its `TypeId`s, so both must come from this same analyzer instance).
        let text = {
            let crate::semantics::analyzer::SemanticInfo { hir, .. } = symbol_info;
            let interner = analyzer.interner();
            let mut mir = crate::mir::lower::lower_program(&hir, interner);
            // Whole-module optimization: tree-shaking + reference-counting insertion + function
            // inlining (see `mir::passes::optimize_module`). RC is inserted there, before inlining,
            // so callee destruction stays deterministic; the per-function pipeline below only cleans
            // up the merged bodies.
            crate::mir::passes::optimize_module(&mut mir, interner);
            let pipeline = crate::mir::passes::PassManager::default_pipeline();
            for f in &mut mir.functions {
                pipeline.run(f, interner);
            }
            match self.target {
                Target::Wasm => crate::mir::emit::emit_module(&mir, interner, self.debug_alloc),
            }
        };

        info!("finished code generation");
        fs::write(out_path, &text)?;
        info!("created file: {}", out_path);

        // Also emit a binary `.wasm` (what browsers/Node load) and an `.abi.json` sidecar
        // describing extern imports and exports so the JS runtime can auto-marshal values.
        emit_wasm_and_abi(out_path, &text, ast.get_root())?;

        Ok(())
    }
}
