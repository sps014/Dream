//! In-memory compilation front-end: lex -> parse -> merge the embedded standard-library
//! prelude -> semantic analysis, collecting diagnostics for a single document. No filesystem
//! access is involved for the prelude (embedded with `include_str!`), so it runs in the browser.

use bumpalo::Bump;
use dream::diagnostics::{Diagnostic, DiagnosticBag, Severity};
use dream::driver::source_loader::collect_declarations;
use dream::semantics::analyzer::Analyzer;
use dream::stdlib::std_package_from_slash_path;
use dream::syntax::lexer::Lexer;
use dream::syntax::nodes::ProgramNode;
use dream::syntax::parser::Parser;
use dream::syntax::syntax_tree::SyntaxTree;

use crate::position::LineIndex;

#[derive(Debug, Clone)]
pub struct DiagnosticOut {
    pub range: crate::position::Range,
    pub severity: &'static str,
    pub message: String,
    /// Stable diagnostic code when known (e.g. `unresolved-name` for auto-import).
    pub code: Option<&'static str>,
}

/// Synthetic file tag for the document under analysis. Diagnostics carrying this tag (or no
/// tag, as produced by the semantic analyzer) belong to the user's code; prelude-tagged
/// diagnostics are filtered out so library-internal spans never map onto the user's text.
pub const MAIN_FILE: &str = "main.dream";

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
        if let Some(module_decl) = &program.module {
            acc.file_modules
                .insert(MAIN_FILE.to_string(), std::rc::Rc::from(module_decl.path.text.as_str()));
        }
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

        for import in &program.imports {
            if import.alias.is_some() {
                continue;
            }
            let module_name = import.module_name.text.as_str();
            if let Some(pkg) = std_package_from_slash_path(module_name) {
                acc.requested_std_packages.insert(pkg.name.to_string());
            }
        }

        if let Some(path_str) = file_path {
            let parent_dir = std::path::Path::new(path_str)
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""));
            acc.visited.insert(path_str.to_string());
            acc.visited.insert(MAIN_FILE.to_string());

            for import in &program.imports {
                if import.alias.is_some() {
                    continue;
                }
                let module_name = import.module_name.text.as_str();
                if std_package_from_slash_path(module_name).is_some() {
                    continue;
                }
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

        if program_uses_json_attr(&acc) {
            acc.requested_std_packages
                .insert("system.json".to_string());
        }
    }

    let _ = dream::driver::prelude::merge_prelude(
        &arena,
        &mut acc.all_functions,
        &mut acc.all_structs,
        &mut acc.all_interfaces,
        &mut acc.all_enums,
        &mut acc.all_extends,
        &mut diagnostics,
        &mut acc.file_contents,
        &mut acc.file_modules,
        &acc.requested_std_packages,
    );

    // Skip compiled-in copies of prelude files the user is actively editing in the compiler repo.
    if let Some(path) = file_path {
        strip_edited_stdlib_duplicates(path, &mut acc);
    }

    dream::attributes::validate_program_attributes(
        &acc.all_structs,
        &acc.all_interfaces,
        &acc.all_functions,
        &acc.all_enums,
        &acc.all_extends,
        &mut diagnostics,
    );

    // Unlike the batch compiler (which stops at the first phase with errors), the editor keeps
    // semantic diagnostics flowing even while the user is mid-edit: the parser recovers and always
    // yields a `ProgramNode`, and the analyzer's poison/`Unknown` type stops a few broken spans
    // from cascading into noise. We only require that the user's document itself parsed into a
    // tree (`user_ast` is `Ok`); a half-formed tree still yields useful semantic diagnostics for
    // the parts that did parse. The analysis is wrapped so any residual panic degrades to
    // "syntax diagnostics only" instead of taking down the language server.
    if user_ast.is_ok() {
        let file_modules: std::collections::HashMap<std::rc::Rc<str>, std::rc::Rc<str>> = acc
            .file_modules
            .iter()
            .map(|(k, v)| (std::rc::Rc::from(k.as_str()), v.clone()))
            .collect();
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
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut analyzer = Analyzer::new(&tree, &arena).with_file_modules(file_modules);
            let _ = analyzer.analyze(&mut diagnostics);
        }));
        if let Err(payload) = result {
            report_analyzer_panic(&mut diagnostics, &payload);
        }
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
                code: diagnostic_code(&d.message),
            })
        })
        .collect()
}

fn diagnostic_code(message: &str) -> Option<&'static str> {
    if message.contains("does not exist")
        || message.contains("not found")
        || message.contains("Function does not exist")
    {
        Some("unresolved-name")
    } else {
        None
    }
}

fn program_uses_json_attr(acc: &dream::driver::source_loader::ProgramAccumulator<'_>) -> bool {
    acc.all_structs.iter().any(|s| {
        s.attributes.iter().any(|a| a.name.text == "json")
    }) || acc.all_enums.iter().any(|e| {
        e.attributes.iter().any(|a| a.name.text == "json")
    })
}

/// When editing a stdlib source file in-tree, drop the embedded twin so definitions don't duplicate.
fn strip_edited_stdlib_duplicates(
    path: &str,
    acc: &mut dream::driver::source_loader::ProgramAccumulator<'_>,
) {
    let norm = path.replace('\\', "/");
    let Some(idx) = norm.find("/src/stdlib/") else {
        return;
    };
    let bare = &norm[idx + "/src/stdlib/".len()..];
    let tag = format!("<std>/{}", bare);
    acc.all_functions
        .retain(|f| f.file_path.as_deref() != Some(tag.as_str()));
    acc.all_structs
        .retain(|s| s.file_path.as_deref() != Some(tag.as_str()));
    acc.all_interfaces
        .retain(|i| i.file_path.as_deref() != Some(tag.as_str()));
    acc.all_enums
        .retain(|e| e.file_path.as_deref() != Some(tag.as_str()));
    acc.all_extends
        .retain(|e| e.file_path.as_deref() != Some(tag.as_str()));
}

/// Extracts a human-readable message from a caught panic payload (`&str`/`String`, or a fallback
/// for anything else, e.g. a custom panic payload type).
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Surfaces a caught analyzer panic instead of silently downgrading to "syntax diagnostics only":
/// logs it (so it shows up in the language server's own logs) and reports it as a warning
/// diagnostic anchored at the start of the document, so the editor visibly indicates that semantic
/// diagnostics for this file may be incomplete. This is a safety net — the compiler's own internal
/// errors (Phase 1 of the fragility cleanup) already turn known-fragile MIR/codegen panics into
/// proper diagnostics before they would ever reach here; this only catches what that pass doesn't.
fn report_analyzer_panic(diagnostics: &mut DiagnosticBag, payload: &(dyn std::any::Any + Send)) {
    let message = panic_message(payload);
    // No structured logger is wired up in this crate; stderr is the language server's own log
    // (VS Code surfaces it in the "Dream Language Server" output channel), separate from the
    // diagnostic pushed below (which is what the *user* sees in-editor).
    eprintln!(
        "[dream-lsp] semantic analyzer panicked (this is a compiler bug): {}",
        message
    );
    diagnostics.diagnostics.push(Diagnostic::warning(
        format!(
            "internal error: semantic analysis crashed while analyzing this file ({}); some \
             diagnostics may be missing. This is a compiler bug, not a problem with your program \
             — please file an issue.",
            message
        ),
        Some(dream::text::text_span::TextSpan {
            start: 0,
            end: 1,
            line_no: 1,
            col_no: 1,
        }),
        None,
    ));
}
