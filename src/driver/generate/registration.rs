//! Generator registration: `@generator_module`, imports, optional `dream.toml` [[generators]].

use dream_diagnostics::DiagnosticBag;
use dream_syntax::lexer::Lexer;
use dream_syntax::parser::Parser;
use bumpalo::Bump;

use crate::driver::source_loader::ProgramAccumulator;

#[derive(Debug, Clone)]
pub struct RegisteredGenerator {
    pub name: String,
    pub module_path: String,
    pub file_path: String,
    pub syntax_blocks: Vec<String>,
}

/// Discovers generators from `@generator_module` files and manifest paths.
pub fn discover_generators(
    acc: &ProgramAccumulator<'_>,
    diagnostics: &mut DiagnosticBag,
) -> Vec<RegisteredGenerator> {
    let mut out = Vec::new();
    let mut paths: Vec<String> = acc.generator_module_files.clone();
    paths.extend(acc.manifest_generator_paths.iter().cloned());
    paths.sort();
    paths.dedup();

    let arena = Bump::new();
    for path in paths {
        let Some(source) = acc.file_contents.get(&path).cloned() else {
            // Manifest path may not be loaded yet — try reading from disk.
            match std::fs::read_to_string(&path) {
                Ok(s) => {
                    let gens = scan_generator_file(&arena, &path, &s, diagnostics);
                    out.extend(gens);
                }
                Err(e) => {
                    diagnostics.report_error(
                        format!("cannot read generator file '{}': {}", path, e),
                        None,
                    );
                }
            }
            continue;
        };
        let gens = scan_generator_file(&arena, &path, &source, diagnostics);
        out.extend(gens);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.file_path.cmp(&b.file_path)));
    out
}

fn scan_generator_file(
    arena: &Bump,
    path: &str,
    source: &str,
    diagnostics: &mut DiagnosticBag,
) -> Vec<RegisteredGenerator> {
    let mut local = DiagnosticBag::new(Some(path.to_string()));
    let lexer = Lexer::new(source.to_string());
    let mut parser = Parser::new(lexer, arena, &mut local);
    let Ok(ast) = parser.parse() else {
        diagnostics.extend(&local);
        return Vec::new();
    };
    diagnostics.extend(&local);
    let program = ast.get_root();
    let module_path = program
        .module
        .as_ref()
        .map(|m| m.path.text.clone())
        .unwrap_or_default();

    let mut out = Vec::new();
    for f in &program.functions {
        let gen_name = f
            .attributes
            .iter()
            .find(|a| a.name.text == "generator")
            .and_then(|a| a.args.first())
            .map(|t| t.text.trim_matches('"').to_string());
        let Some(name) = gen_name else {
            continue;
        };
        let syntax_blocks: Vec<String> = f
            .attributes
            .iter()
            .filter(|a| a.name.text == "syntax_block")
            .filter_map(|a| a.args.first().map(|t| t.text.trim_matches('"').to_string()))
            .collect();
        out.push(RegisteredGenerator {
            name,
            module_path: module_path.clone(),
            file_path: path.to_string(),
            syntax_blocks,
        });
    }
    out
}
