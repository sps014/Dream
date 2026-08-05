use bumpalo::Bump;
use std::fs;
use tracing::{info, warn};

use crate::diagnostics::{render, DiagnosticBag};
use crate::driver::abi::emit_wasm_and_abi;
use crate::driver::error::CompileError;
use crate::driver::json_derive::generate_json_derives;
use crate::driver::prelude::merge_prelude;
use crate::driver::source_loader::{parse_file_recursive, ProgramAccumulator};
use crate::driver::wasm_opt::OptLevel;
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
    /// When `true` (the default), codegen emits allocator instrumentation so the
    /// `Debug.live_objects()` / `Debug.total_allocations()` probes report real values, and keeps
    /// every runtime helper in the WAT (skips structural dead-function elimination). Release builds
    /// (`--release` / [`Compiler::with_release`]) turn this off for a trimmed, uninstrumented module.
    debug: bool,
    /// When `true`, the compiler threads source-line info through HIR/MIR and the backend emits
    /// source-line hooks + a `.dbg.json` source map for the interactive debugger. Off by default;
    /// enabled via the CLI `-g`/`--debug-info` flag or [`Compiler::with_debug_info`].
    debug_info: bool,
    /// When set, the emitted `.wasm` is post-processed in place with Binaryen's `wasm-opt` at this
    /// level. [`Compiler::with_release`] enables [`OptLevel::RELEASE_DEFAULT`] when no level was
    /// set yet; an explicit [`Compiler::with_optimize`] (or CLI `-O`) overrides that default.
    /// Debug builds leave this `None` unless the caller opts in.
    optimize: Option<OptLevel>,
}

impl Compiler {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            debug: true,
            debug_info: false,
            optimize: None,
        }
    }

    /// Builder: when `on` is `true`, produce a release module — uninstrumented allocator, structural
    /// WAT dead-function elimination (`strip_dead_functions`), and wasm-opt at
    /// [`OptLevel::RELEASE_DEFAULT`] unless a level was already set via [`Compiler::with_optimize`].
    /// When `false` (the default from [`Compiler::new`]), keep allocator probes and the full runtime
    /// (does not clear a previously configured optimize level).
    pub fn with_release(mut self, on: bool) -> Self {
        self.debug = !on;
        if on && self.optimize.is_none() {
            self.optimize = Some(OptLevel::RELEASE_DEFAULT);
        }
        self
    }

    /// Builder: enable source-level debug-info instrumentation (line hooks + source map) for the
    /// interactive debugger.
    pub fn with_debug_info(mut self, on: bool) -> Self {
        self.debug_info = on;
        self
    }

    /// Builder: post-process the emitted `.wasm` with Binaryen's `wasm-opt` at the given level.
    /// `Some(level)` sets/overrides (including the [`OptLevel::RELEASE_DEFAULT`] from
    /// [`Compiler::with_release`]); `None` clears post-processing entirely.
    pub fn with_optimize(mut self, level: Option<OptLevel>) -> Self {
        self.optimize = level;
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

        // Validate every attribute in the merged program (unknown names, disallowed placements,
        // wrong argument shapes, duplicates) before anything downstream (the `@json` derive below,
        // then semantic analysis) reads attributes assuming they are well-formed.
        crate::attributes::validate_program_attributes(
            &acc.all_structs,
            &acc.all_interfaces,
            &acc.all_functions,
            &acc.all_enums,
            &acc.all_extends,
            &mut diagnostics,
        );
        if diagnostics.has_errors() {
            render(&diagnostics, &acc.file_contents);
            return Err(CompileError::Syntax);
        }

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

        // Inherit interface default-method bodies into implementing classes that omit them, by
        // appending synthesized `extend` blocks (must run after class collection so `implements`
        // clauses are all present).
        crate::driver::interface_defaults::generate_interface_default_impls(
            &acc.all_structs,
            &acc.all_interfaces,
            &mut acc.all_extends,
        );

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

        let file_modules: std::collections::HashMap<std::rc::Rc<str>, std::rc::Rc<str>> = acc
            .file_modules
            .iter()
            .map(|(file, module)| (std::rc::Rc::from(file.as_str()), module.clone()))
            .collect();
        let mut analyzer = Analyzer::new(&ast, &arena)
            .with_file_modules(file_modules)
            .with_aliased_imports(acc.aliased_imports);
        analyzer.set_debug_info(self.debug_info);
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
        let crate::semantics::analyzer::SemanticInfo { hir, .. } = symbol_info;
        let interner = analyzer.interner();
        let target = &self.target;
        let debug = self.debug;
        let debug_info = self.debug_info;

        // Codegen (MIR lowering/optimization/emission) treats certain lookups - a type's layout, an
        // interned string, a function-table slot - as compiler invariants rather than user errors: a
        // miss means the analyzer and codegen disagree about a well-typed program, i.e. a compiler
        // bug (see `crate::internal_error!`). Catching the resulting panic here turns that into a
        // clean, typed `CompileError::Internal` instead of an unwinding panic with a raw backtrace
        // reaching the user.
        // Suppress the default panic hook's raw "thread 'main' panicked at ..." dump for the
        // duration of this call: the panic is already a well-formed internal-error message (see
        // `render_internal_error` below), and we don't want a Rust backtrace header confusing users
        // who never expect to see a stack trace from a compiler CLI.
        // `take_hook`/`set_hook` operate on process-global state, so concurrent `compile()` calls
        // (e.g. the e2e corpus running on a rayon pool) racing on this swap can permanently clobber
        // the hook with another thread's no-op — the mutex below serializes the swap+restore so each
        // call's `take_hook` always observes the real previous hook.
        static HOOK_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let hook_guard = HOOK_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let codegen_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut mir = crate::mir::lower::lower_program(&hir, interner);
            // Whole-module optimization: tree-shaking + reference-counting insertion + function
            // inlining (see `mir::passes::optimize_module`). RC is inserted there, before inlining,
            // so callee destruction stays deterministic; the per-function pipeline below only cleans
            // up the merged bodies.
            // Debug-info builds skip inlining and use a value-preserving per-function pipeline so
            // user variables and per-function call frames survive for the debugger; release builds
            // use the full optimizing pipeline.
            crate::mir::passes::optimize_module_opts(&mut mir, interner, !debug_info);
            let pipeline = if debug_info {
                crate::mir::passes::PassManager::debug_pipeline()
            } else {
                crate::mir::passes::PassManager::default_pipeline()
            };
            for f in &mut mir.functions {
                pipeline.run(f, interner);
            }
            match target {
                Target::Wasm => {
                    crate::mir::emit::emit_module_with_debug(&mir, interner, debug, debug_info)
                }
            }
        }));
        std::panic::set_hook(previous_hook);
        drop(hook_guard);

        let (text, debug_map) = codegen_result.map_err(|panic_payload| {
            let message = panic_message(&panic_payload);
            render_internal_error(&message);
            CompileError::Internal(message)
        })?;

        info!("finished code generation");
        fs::write(out_path, &text)?;
        info!("created file: {}", out_path);

        // Emit the debug-info source map next to the `.wat` when debug-info is enabled, so the
        // interactive debugger can map hook calls back to source lines/variables.
        if let Some(map) = debug_map {
            let map_path = debug_map_path(out_path);
            fs::write(&map_path, map.to_json())?;
            info!("created debug map: {}", map_path);
        }

        // Also emit a binary `.wasm` (what browsers/Node load) and an `.abi.json` sidecar
        // describing extern imports and exports so the JS runtime can auto-marshal values.
        emit_wasm_and_abi(out_path, &text, ast.get_root())?;

        if let Some(level) = self.optimize {
            let wasm_path = std::path::Path::new(out_path).with_extension("wasm");
            // Non-fatal, matching the existing `.wasm` assembly failure handling in
            // `emit_wasm_and_abi`: the unoptimized `.wasm`/`.wat` are already valid output.
            match crate::driver::wasm_opt::optimize_wasm_file(&wasm_path, level) {
                Ok(()) => info!("optimized file with wasm-opt: {}", wasm_path.display()),
                Err(e) => warn!("could not run wasm-opt on {}: {}", wasm_path.display(), e),
            }
        }

        Ok(())
    }
}

/// Extracts a human-readable message from a caught panic payload (the `Any` that
/// `std::panic::catch_unwind` hands back), covering the two shapes `panic!`/`internal_error!`
/// actually produce (`&'static str` and `String`).
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "internal compiler error: codegen panicked with a non-string payload".to_string()
    }
}

/// Prints a caught codegen panic the way [`render`] prints ordinary diagnostics, so an internal
/// compiler error looks like the rest of the CLI's output rather than a raw Rust panic dump.
fn render_internal_error(message: &str) {
    eprintln!("error: {}", message);
}

/// Derives the `.dbg.json` debug-map path that sits next to the compiled `.wat` output.
fn debug_map_path(out_path: &str) -> String {
    let path = std::path::Path::new(out_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    parent
        .join(format!("{}.dbg.json", stem))
        .to_string_lossy()
        .into_owned()
}
