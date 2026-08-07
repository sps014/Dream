use dream::driver::compiler::{Compiler, Target};
use dream::driver::js_runtime::JsRuntimeTarget;
use dream::driver::wasm_opt::OptLevel;
use dream::execution::wasm_runner::execute_wasm;
use std::path::Path;
use std::process::ExitCode;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

/// Returns a non-zero [`ExitCode`] on any failure (bad arguments, invalid path, compile error, or
/// run error) so CI pipelines and shell scripts can detect and react to failures. `--help`/`-h`
/// prints usage and exits successfully.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let program = args
        .first()
        .map(String::as_str)
        .unwrap_or("dream")
        .to_string();

    let mut verbose = false;
    let mut run_after_compile = false;
    let mut release = false;
    let mut debug_info = false;
    let mut debug_adapter = false;
    let mut show_help = false;
    let mut file_name = None;
    let mut optimize: Option<OptLevel> = None;
    let mut want_runtime = false;
    let mut runtime_target: Option<JsRuntimeTarget> = None;

    for arg in args.iter().skip(1) {
        if arg == "-v" || arg == "--verbose" {
            verbose = true;
        } else if arg == "--release" {
            // Trimmed release build: uninstrumented allocator + structural WAT dead-function
            // elimination + wasm-opt at OptLevel::RELEASE_DEFAULT (-Os) unless -O overrides.
            // Default (no flag) keeps allocator probes and the full runtime.
            release = true;
        } else if arg == "-g" || arg == "--debug-info" {
            // Enable source-level debug-info: line hooks + a `.dbg.json` source map for the
            // interactive debugger. Off by default (zero overhead in normal builds). Combined with
            // `--release`, allocator instrumentation is still off, but WAT DCE stays disabled
            // because the debugger needs the full module.
            debug_info = true;
        } else if arg == "-h" || arg == "--help" {
            show_help = true;
        } else if arg == "run" {
            run_after_compile = true;
        } else if arg == "debug-adapter" {
            // Speak the Debug Adapter Protocol over stdio for the given source file (used by editor
            // debug clients such as the VS Code extension). Implies debug-info.
            debug_adapter = true;
            debug_info = true;
        } else if arg == "--runtime" {
            want_runtime = true;
        } else if arg == "--web" {
            runtime_target = Some(JsRuntimeTarget::Web);
        } else if arg == "--node" {
            runtime_target = Some(JsRuntimeTarget::Node);
        } else if arg == "-O" || arg == "--optimize" {
            // No level given: default to `-Os` (optimize for size), matching the "smaller binary"
            // intent most users reach for this flag with. Also overrides `--release`'s default.
            optimize = Some(OptLevel::Size);
        } else if let Some(level_str) = arg.strip_prefix("--optimize=") {
            match level_str.parse::<OptLevel>() {
                Ok(level) => optimize = Some(level),
                Err(e) => {
                    error!("{}", e);
                    return ExitCode::FAILURE;
                }
            }
        } else if let Some(level_str) = arg.strip_prefix("-O") {
            let level_str = level_str.strip_prefix('=').unwrap_or(level_str);
            match level_str.parse::<OptLevel>() {
                Ok(level) => optimize = Some(level),
                Err(e) => {
                    error!("{}", e);
                    return ExitCode::FAILURE;
                }
            }
        } else if !arg.starts_with('-') {
            file_name = Some(arg);
        }
    }

    if (release || optimize.is_some()) && !cfg!(feature = "wasm-opt") {
        error!(
            "--release / -O/--optimize requires the compiler to be built with the `wasm-opt` feature \
             (enabled by default); this build was compiled without it"
        );
        return ExitCode::FAILURE;
    }

    if want_runtime && runtime_target.is_none() {
        error!("--runtime requires --web or --node");
        print_usage(&program);
        return ExitCode::FAILURE;
    }
    if runtime_target.is_some() && !want_runtime {
        error!("--web / --node require --runtime");
        print_usage(&program);
        return ExitCode::FAILURE;
    }

    // Route logs to stderr so they never corrupt stdout — critical in `debug-adapter` mode, where
    // stdout carries the framed DAP protocol stream (and harmless/conventional for other modes).
    let subscriber = FmtSubscriber::builder()
        .with_max_level(if verbose { Level::INFO } else { Level::WARN })
        .without_time()
        .with_target(false)
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    if show_help {
        print_usage(&program);
        return ExitCode::SUCCESS;
    }

    let file_name = match file_name {
        Some(name) => name,
        None => {
            error!("Expected a source file (*.dream) as argument");
            print_usage(&program);
            return ExitCode::FAILURE;
        }
    };

    info!("Dream Compiler Tools");
    info!("========================");
    info!("Compiling file: {}", file_name);

    // `with_release` installs RELEASE_DEFAULT wasm-opt; an explicit `-O` overrides. Do not call
    // `with_optimize(None)` after release — that would clear the default.
    // JS hosts need `.abi.json` (compile-only or `--runtime`); native `run` / `debug-adapter` skip it.
    let emit_abi = want_runtime || !(run_after_compile || debug_adapter);
    let mut compiler = Compiler::new(Target::Wasm)
        .with_release(release)
        .with_debug_info(debug_info)
        .with_runtime(runtime_target)
        .with_emit_abi(emit_abi);
    if let Some(level) = optimize {
        compiler = compiler.with_optimize(Some(level));
    }
    let out_path = match get_path_from_file_path(file_name) {
        Some(path) => path,
        None => {
            error!("Invalid source file path: {}", file_name);
            return ExitCode::FAILURE;
        }
    };

    match compiler.compile(file_name, &out_path) {
        Ok(_) => {
            info!("Compilation successful");

            if debug_adapter {
                // Hand control to the Debug Adapter Protocol server, which loads the just-emitted
                // `.wat` + `.dbg.json` and drives execution under the debugger over stdio.
                if let Err(e) = dream::execution::debugger::run_debug_adapter(&out_path) {
                    error!("Debug adapter failed: {}", e);
                    return ExitCode::FAILURE;
                }
                return ExitCode::SUCCESS;
            }

            if run_after_compile {
                info!("Executing via Wasmtime...");
                if let Err(e) = execute_wasm(&out_path) {
                    error!("Execution failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("Compilation failed: {}", e.to_string());
            ExitCode::FAILURE
        }
    }
}

/// Prints CLI usage to stderr via the tracing subscriber's error channel.
fn print_usage(program: &str) {
    error!(
        "Usage: {} [-v|--verbose] [--release] [-g|--debug-info] [-O|--optimize[=LEVEL]] [--runtime --web|--node] [run|debug-adapter] <file>",
        program
    );
    error!("  -v, --verbose         Print progress information");
    error!(
        "  --release             Trimmed build + wasm-opt (-Os); override level with -O"
    );
    error!(
        "  -g, --debug-info      Emit source-level debug info (line hooks + .dbg.json source map)"
    );
    error!(
        "  -O, --optimize[=LVL]  wasm-opt level (LVL: 0-4, s, z; default: s); overrides --release"
    );
    error!(
        "  --runtime             Emit a tree-shaken sibling *.runtime.js (requires --web or --node)"
    );
    error!("  --web                 With --runtime: browser-targeted *.runtime.js");
    error!("  --node                With --runtime: Node-targeted *.runtime.js");
    error!("  -h, --help            Show this help message");
    error!("  run                   Execute the compiled module after a successful build");
    error!("  debug-adapter         Run the Debug Adapter Protocol server over stdio (implies -g)");
    error!(r"Example: {} run src/sample/test_arrays.dream", program);
    error!(r"Example: {} --release run src/sample/test_arrays.dream", program);
    error!(r"Example: {} --runtime --web sample/interop/js.dream", program);
    error!(r"Example: {} --runtime --node sample/interop/js.dream", program);
}

/// Derives the output `.wat` path that sits next to the given source file.
/// Returns `None` if the path has no file stem or contains non-UTF-8 components.
fn get_path_from_file_path(file_path: &str) -> Option<String> {
    let path = Path::new(file_path);
    let file_stem = path.file_stem()?.to_str()?;
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let result = parent.join(format!("{}.wat", file_stem));
    Some(result.to_str()?.to_string())
}
