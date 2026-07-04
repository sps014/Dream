use dream::driver::compiler::{Compiler, Target};
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
    let mut debug = false;
    let mut debug_info = false;
    let mut debug_adapter = false;
    let mut show_help = false;
    let mut file_name = None;

    for arg in args.iter().skip(1) {
        if arg == "-v" || arg == "--verbose" {
            verbose = true;
        } else if arg == "-d" || arg == "--debug" {
            // Enable allocator instrumentation so the `Debug.live_objects()` /
            // `Debug.total_allocations()` probes report real values. Off by default so normal
            // builds carry zero per-allocation overhead.
            debug = true;
        } else if arg == "-g" || arg == "--debug-info" {
            // Enable source-level debug-info: line hooks + a `.dbg.json` source map for the
            // interactive debugger. Off by default (zero overhead in normal builds).
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
        } else if !arg.starts_with('-') {
            file_name = Some(arg);
        }
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

    let compiler = Compiler::new(Target::Wasm)
        .with_debug(debug)
        .with_debug_info(debug_info);
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
        "Usage: {} [-v|--verbose] [-d|--debug] [-g|--debug-info] [run|debug-adapter] <file>",
        program
    );
    error!("  -v, --verbose      Print progress information");
    error!("  -d, --debug        Enable allocator instrumentation for Debug probes");
    error!("  -g, --debug-info   Emit source-level debug info (line hooks + .dbg.json source map)");
    error!("  -h, --help         Show this help message");
    error!("  run                Execute the compiled module after a successful build");
    error!("  debug-adapter      Run the Debug Adapter Protocol server over stdio (implies -g)");
    error!(r"Example: {} run src/sample/test_arrays.dream", program);
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
