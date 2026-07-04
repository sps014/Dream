//! In-memory compilation front-end: lex -> parse -> merge the embedded standard-library
//! prelude -> semantic analysis, collecting diagnostics for a single document. No filesystem
//! access is involved (the prelude is embedded with `include_str!`), so it runs in the browser.

use bumpalo::Bump;
use dream::diagnostics::{DiagnosticBag, Severity};
use dream::driver::source_loader::collect_declarations;
use dream::semantics::analyzer::Analyzer;
use dream::syntax::lexer::Lexer;
use dream::syntax::nodes::struct_node::StructDeclarationNode;
use dream::syntax::nodes::{ExtendNode, FunctionNode, ProgramNode};
use dream::syntax::parser::Parser;
use dream::syntax::syntax_tree::SyntaxTree;

use crate::position::LineIndex;

#[derive(Debug, Clone)]
pub struct DiagnosticOut {
    pub range: crate::position::Range,
    pub severity: &'static str,
    pub message: String,
}

/// Synthetic file tag for the document under analysis. Diagnostics carrying this tag (or no
/// tag, as produced by the semantic analyzer) belong to the user's code; prelude-tagged
/// diagnostics are filtered out so library-internal spans never map onto the user's text.
pub const MAIN_FILE: &str = "main.dream";

/// The embedded standard-library prelude. Re-exported from the compiler crate so the language
/// service and the compiler can never drift (see `dream::stdlib::PRELUDE_FILES`).
use dream::stdlib::PRELUDE_FILES;

/// Runs the full front-end over `text` and returns the diagnostics that belong to the user's
/// document, with byte spans converted to LSP ranges.
pub fn collect_diagnostics(file_path: Option<&str>, text: &str) -> Vec<DiagnosticOut> {
    let arena = Bump::new();
    let line_index = LineIndex::new(text);

    let mut diagnostics = DiagnosticBag::new(None);

    let mut acc = dream::driver::source_loader::ProgramAccumulator::default();

    // Parse the user's document. Parsing reports lexical/syntactic errors into `user_bag`.
    let mut user_bag = DiagnosticBag::new(Some(MAIN_FILE.to_string()));
    let user_ast = {
        let lexer = Lexer::new(text.to_string());
        let mut parser = Parser::new(lexer, &arena, &mut user_bag);
        parser.parse()
    };
    diagnostics.extend(&user_bag);

    if let Ok(ast) = &user_ast {
        let program = ast.get_root();
        collect_declarations(
            program,
            MAIN_FILE,
            &mut acc.all_functions,
            &mut acc.all_structs,
            &mut acc.all_interfaces,
            &mut acc.all_enums,
            &mut acc.all_extends,
            &mut acc.all_globals,
        );

        if let Some(path_str) = file_path {
            let parent_dir = std::path::Path::new(path_str)
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""));
            acc.visited.insert(path_str.to_string());
            acc.visited.insert(MAIN_FILE.to_string());

            for import in &program.imports {
                let module_name = import.module_name.text.as_str();
                let import_path =
                    dream::driver::source_loader::resolve_import_path(parent_dir, module_name);

                if let Some(import_path_str) = import_path.to_str() {
                    if import_path.exists() {
                        let _ = dream::driver::source_loader::parse_file_recursive(
                            &import_path_str.to_string(),
                            &mut acc,
                            &arena,
                            &mut diagnostics,
                        );
                    }
                }
            }
        }
    }

    merge_prelude(
        &arena,
        file_path,
        &mut diagnostics,
        &mut acc.all_functions,
        &mut acc.all_structs,
        &mut acc.all_interfaces,
        &mut acc.all_enums,
        &mut acc.all_extends,
    );

    // Unlike the batch compiler (which stops at the first phase with errors), the editor keeps
    // semantic diagnostics flowing even while the user is mid-edit: the parser recovers and always
    // yields a `ProgramNode`, and the analyzer's poison/`Unknown` type stops a few broken spans
    // from cascading into noise. We only require that the user's document itself parsed into a
    // tree (`user_ast` is `Ok`); a half-formed tree still yields useful semantic diagnostics for
    // the parts that did parse. The analysis is wrapped so any residual panic degrades to
    // "syntax diagnostics only" instead of taking down the language server.
    if user_ast.is_ok() {
        let combined = ProgramNode::new(
            vec![],
            acc.all_structs,
            acc.all_interfaces,
            acc.all_functions,
            acc.all_enums,
            acc.all_extends,
            acc.all_globals,
        );
        let tree = SyntaxTree::new(combined);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut analyzer = Analyzer::new(&tree, &arena);
            let _ = analyzer.analyze(&mut diagnostics);
        }));
    }

    diagnostics
        .diagnostics
        .iter()
        .filter(|d| matches!(d.file_path.as_deref(), None | Some(MAIN_FILE)))
        .filter_map(|d| {
            let span = d.span?;
            // Guard against synthesized zero spans pointing outside the document.
            if span.start > text.len() {
                return None;
            }
            let end = if span.end > span.start {
                span.end
            } else {
                span.start + 1
            };
            Some(DiagnosticOut {
                range: line_index.range(span.start, end),
                severity: match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                },
                message: d.message.clone(),
            })
        })
        .collect()
}

/// Parses each embedded prelude file and merges its declarations, tagging them with their
/// `<std>` path so their diagnostics can be filtered out of the user-facing list.
// The many parameters are parallel per-declaration-kind accumulators; grouping them into a struct
// would just move the same field list elsewhere without improving call sites.
#[allow(clippy::too_many_arguments)]
fn merge_prelude<'a>(
    arena: &'a Bump,
    file_path: Option<&str>,
    diagnostics: &mut DiagnosticBag,
    all_functions: &mut Vec<FunctionNode<'a>>,
    all_structs: &mut Vec<StructDeclarationNode<'a>>,
    all_interfaces: &mut Vec<dream::syntax::nodes::InterfaceDeclarationNode<'a>>,
    all_enums: &mut Vec<dream::syntax::nodes::EnumDeclarationNode>,
    all_extends: &mut Vec<ExtendNode<'a>>,
) {
    for &(name, src) in PRELUDE_FILES {
        // If the user is actively editing this prelude file in the compiler repo, skip merging
        // the compiled-in version so we don't get duplicate definition errors in the editor.
        if let Some(path) = file_path {
            let bare_name = name.trim_start_matches("<std>/");
            if path
                .replace('\\', "/")
                .ends_with(&format!("/src/stdlib/{}", bare_name))
            {
                continue;
            }
        }

        let mut prelude_bag = DiagnosticBag::new(Some(name.to_string()));
        let lexer = Lexer::new(src.to_string());
        let mut parser = Parser::new(lexer, arena, &mut prelude_bag);
        let parsed = parser.parse();
        diagnostics.extend(&prelude_bag);

        if let Ok(ast) = parsed {
            let mut globals = Vec::new();
            collect_declarations(
                ast.get_root(),
                name,
                all_functions,
                all_structs,
                all_interfaces,
                all_enums,
                all_extends,
                &mut globals,
            );
        }
    }
}
