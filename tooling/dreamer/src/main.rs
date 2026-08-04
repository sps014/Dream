use clap::{Parser, Subcommand};
use dreamer::commands;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "dreamer",
    version,
    about = "Package manager for the Dream language"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a new dream.toml + src/main.dream in the current (or given) directory.
    Init {
        /// Project name; defaults to the directory name.
        name: Option<String>,
        /// Directory to create the project in (created if missing). Defaults to the current directory.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Add a dependency to dream.toml, then resolve and install it.
    Add {
        name: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        git: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        rev: Option<String>,
        /// Add under [dev-dependencies] instead of [dependencies].
        #[arg(long)]
        dev: bool,
    },
    /// Remove a dependency from dream.toml and dream_packages/.
    Remove { name: String },
    /// Resolve dream.toml into dream.lock and materialize dream_packages/.
    Install,
    /// Re-resolve dependencies to the latest compatible versions.
    Update {
        /// Update only this package (other pins are kept as-is).
        name: Option<String>,
    },
    /// Install dependencies, then compile the project's entry point.
    Build {
        /// Post-process the emitted .wasm with wasm-opt (passed through as `-O` to `dream`).
        #[arg(long)]
        release: bool,
    },
    /// Install dependencies, then compile and execute the project's entry point.
    Run {
        /// Extra arguments forwarded to the compiled program.
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Package the current project and publish it to a registry.
    Publish {
        /// Registry base URL; defaults to [registries] default in dream.toml.
        #[arg(long)]
        registry: Option<String>,
    },
    /// Search a registry for packages by name.
    Search { query: String },
    /// Print the resolved dependency tree from dream.lock.
    Tree,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: could not determine current directory: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let result = match cli.command {
        Cmd::Init { name, dir } => commands::init::run(&dir.unwrap_or(cwd), name),
        Cmd::Add {
            name,
            version,
            path,
            git,
            tag,
            branch,
            rev,
            dev,
        } => commands::add::run(&cwd, name, version, path, git, tag, branch, rev, dev),
        Cmd::Remove { name } => commands::remove::run(&cwd, &name),
        Cmd::Install => commands::install::run(&cwd),
        Cmd::Update { name } => commands::update::run(&cwd, name),
        Cmd::Build { release } => commands::build::run(&cwd, release),
        Cmd::Run { args } => commands::run::run(&cwd, &args),
        Cmd::Publish { registry } => commands::publish::run(&cwd, registry),
        Cmd::Search { query } => commands::search::run(&cwd, &query),
        Cmd::Tree => commands::tree::run(&cwd),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {:#}", e);
            ExitCode::FAILURE
        }
    }
}
