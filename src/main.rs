use dream::driver::compiler::{Compiler, Target};
use dream::driver::js_runtime::JsRuntimeTarget;
use dream::driver::wasm_opt::OptLevel;
use dream::execution::wasm_runner::execute_wasm;
use dream_sema::analyzer::CrateType;
use std::path::{Path, PathBuf};
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
    let mut want_web = false;
    let mut want_node = false;
    let mut crate_type = CrateType::Bin;
    let mut crate_type_explicit = false;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
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
            want_web = true;
        } else if arg == "--node" {
            want_node = true;
        } else if arg == "--crate-type" {
            i += 1;
            let Some(val) = args.get(i) else {
                error!("--crate-type requires lib or bin");
                return ExitCode::FAILURE;
            };
            match val.as_str() {
                "lib" => crate_type = CrateType::Lib,
                "bin" => crate_type = CrateType::Bin,
                other => {
                    error!("unknown --crate-type '{}': expected lib or bin", other);
                    return ExitCode::FAILURE;
                }
            }
            crate_type_explicit = true;
        } else if let Some(val) = arg.strip_prefix("--crate-type=") {
            match val {
                "lib" => crate_type = CrateType::Lib,
                "bin" => crate_type = CrateType::Bin,
                other => {
                    error!("unknown --crate-type '{}': expected lib or bin", other);
                    return ExitCode::FAILURE;
                }
            }
            crate_type_explicit = true;
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
        i += 1;
    }
    let _ = crate_type_explicit;

    if (release || optimize.is_some()) && !cfg!(feature = "wasm-opt") {
        error!(
            "--release / -O/--optimize requires the compiler to be built with the `wasm-opt` feature \
             (enabled by default); this build was compiled without it"
        );
        return ExitCode::FAILURE;
    }

    let mut runtimes = Vec::new();
    if want_web {
        runtimes.push(JsRuntimeTarget::Web);
    }
    if want_node {
        runtimes.push(JsRuntimeTarget::Node);
    }

    if want_runtime && runtimes.is_empty() {
        error!("--runtime requires --web and/or --node");
        print_usage(&program);
        return ExitCode::FAILURE;
    }
    if !runtimes.is_empty() && !want_runtime {
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
        .with_runtimes(runtimes)
        .with_emit_abi(emit_abi)
        .with_crate_type(crate_type);
    if let Some(level) = optimize {
        compiler = compiler.with_optimize(Some(level));
    }
    let out_path = match get_path_from_file_path(file_name, release) {
        Some(path) => path,
        None => {
            error!("Invalid source file path: {}", file_name);
            return ExitCode::FAILURE;
        }
    };

    if let Some(parent) = Path::new(&out_path).parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                error!("could not create output directory {}: {}", parent.display(), e);
                return ExitCode::FAILURE;
            }
        }
    }

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
        "Usage: {} [-v|--verbose] [--release] [-g|--debug-info] [-O|--optimize[=LEVEL]] [--crate-type lib|bin] [--runtime --web|--node] [run|debug-adapter] <file>",
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
    error!("  --crate-type lib|bin  Library (no primary main) or binary (default: bin)");
    error!(
        "  --runtime             Emit tree-shaken *.(web|node).runtime.js (requires --web and/or --node)"
    );
    error!("  --web                 With --runtime: browser-targeted *.web.runtime.js");
    error!("  --node                With --runtime: Node-targeted *.node.runtime.js");
    error!("  -h, --help            Show this help message");
    error!("  run                   Execute the compiled module after a successful build");
    error!("  debug-adapter         Run the Debug Adapter Protocol server over stdio (implies -g)");
    error!(r"Example: {} run src/sample/test_arrays.dream", program);
    error!(r"Example: {} --release run src/sample/test_arrays.dream", program);
    error!(r"Example: {} --runtime --web sample/interop/js.dream", program);
    error!(r"Example: {} --runtime --node sample/interop/js.dream", program);
    error!(
        r"Example: {} --runtime --web --node sample/interop/js.dream",
        program
    );
}

/// Walk upward from a file's directory looking for `dream.toml`.
fn find_project_root(file_path: &Path) -> Option<PathBuf> {
    let mut dir = file_path.parent().map(Path::to_path_buf)?;
    loop {
        if dir.join("dream.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Derives the output `.wat` path.
///
/// When a `dream.toml` encloses the source file, artifacts go under
/// `target/debug/` or `target/release/` at the project root. Otherwise they sit beside the source.
fn get_path_from_file_path(file_path: &str, release: bool) -> Option<String> {
    let path = Path::new(file_path);
    let file_stem = path.file_stem()?.to_str()?;
    let out_dir = if let Some(root) = find_project_root(path) {
        let profile = if release { "release" } else { "debug" };
        root.join("target").join(profile)
    } else {
        path.parent().unwrap_or_else(|| Path::new("")).to_path_buf()
    };
    let result = out_dir.join(format!("{}.wat", file_stem));
    Some(result.to_str()?.to_string())
}
