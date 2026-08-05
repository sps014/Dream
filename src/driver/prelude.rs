//! Standard-library prelude merging. Each built-in type lives in its own embedded prelude file
//! (`dream_stdlib`); their declarations are parsed with the user's arena and merged into the
//! program so the built-in types are real, extensible classes.

use bumpalo::Bump;
use indexmap::IndexSet;
use std::collections::HashMap;
use std::io::Error;
use std::rc::Rc;

use dream_diagnostics::DiagnosticBag;
use crate::driver::source_loader::collect_declarations;
use dream_stdlib::{resolve_packages_to_load, StdPackage};
use dream_syntax::lexer::Lexer;
use dream_syntax::parser::Parser;

/// Parses the requested embedded stdlib packages (bootstrap + `requested` + transitive deps) and
/// merges their declarations into the program. Uses the same arena as the user's files so all AST
/// nodes share a lifetime.
#[allow(clippy::too_many_arguments)]
pub fn merge_prelude<'a>(
    arena: &'a Bump,
    all_functions: &mut Vec<dream_syntax::nodes::FunctionNode<'a>>,
    all_structs: &mut Vec<dream_syntax::nodes::struct_node::StructDeclarationNode<'a>>,
    all_interfaces: &mut Vec<dream_syntax::nodes::InterfaceDeclarationNode<'a>>,
    all_enums: &mut Vec<dream_syntax::nodes::EnumDeclarationNode>,
    all_extends: &mut Vec<dream_syntax::nodes::ExtendNode<'a>>,
    diagnostics: &mut DiagnosticBag,
    file_contents: &mut HashMap<String, String>,
    file_modules: &mut HashMap<String, Rc<str>>,
    requested_packages: &IndexSet<String>,
) -> Result<(), Error> {
    let packages = resolve_packages_to_load(requested_packages);
    for pkg in packages {
        merge_package(
            pkg,
            arena,
            all_functions,
            all_structs,
            all_interfaces,
            all_enums,
            all_extends,
            diagnostics,
            file_contents,
            file_modules,
        )?;
    }
    Ok(())
}

/// Merges every stdlib package (full surface). Used by host scans and callers that need the
/// complete prelude regardless of user imports.
#[allow(clippy::too_many_arguments)]
pub fn merge_full_prelude<'a>(
    arena: &'a Bump,
    all_functions: &mut Vec<dream_syntax::nodes::FunctionNode<'a>>,
    all_structs: &mut Vec<dream_syntax::nodes::struct_node::StructDeclarationNode<'a>>,
    all_interfaces: &mut Vec<dream_syntax::nodes::InterfaceDeclarationNode<'a>>,
    all_enums: &mut Vec<dream_syntax::nodes::EnumDeclarationNode>,
    all_extends: &mut Vec<dream_syntax::nodes::ExtendNode<'a>>,
    diagnostics: &mut DiagnosticBag,
    file_contents: &mut HashMap<String, String>,
    file_modules: &mut HashMap<String, Rc<str>>,
) -> Result<(), Error> {
    let all: IndexSet<String> = dream_stdlib::STD_PACKAGES
        .iter()
        .map(|p| p.name.to_string())
        .collect();
    merge_prelude(
        arena,
        all_functions,
        all_structs,
        all_interfaces,
        all_enums,
        all_extends,
        diagnostics,
        file_contents,
        file_modules,
        &all,
    )
}

#[allow(clippy::too_many_arguments)]
fn merge_package<'a>(
    pkg: &StdPackage,
    arena: &'a Bump,
    all_functions: &mut Vec<dream_syntax::nodes::FunctionNode<'a>>,
    all_structs: &mut Vec<dream_syntax::nodes::struct_node::StructDeclarationNode<'a>>,
    all_interfaces: &mut Vec<dream_syntax::nodes::InterfaceDeclarationNode<'a>>,
    all_enums: &mut Vec<dream_syntax::nodes::EnumDeclarationNode>,
    all_extends: &mut Vec<dream_syntax::nodes::ExtendNode<'a>>,
    diagnostics: &mut DiagnosticBag,
    file_contents: &mut HashMap<String, String>,
    file_modules: &mut HashMap<String, Rc<str>>,
) -> Result<(), Error> {
    for &(prelude_name, prelude_src) in pkg.files {
        let prelude_name = prelude_name.to_string();
        file_contents.insert(prelude_name.clone(), prelude_src.to_string());

        let mut prelude_diagnostics = DiagnosticBag::new(Some(prelude_name.clone()));
        let lexer = Lexer::new(prelude_src.to_string());
        let mut parser = Parser::new(lexer, arena, &mut prelude_diagnostics);
        let ast = match parser.parse() {
            Ok(ast) => ast,
            Err(e) => {
                diagnostics.extend(&prelude_diagnostics);
                return Err(e);
            }
        };
        diagnostics.extend(&prelude_diagnostics);

        let program = ast.get_root();
        if let Some(module_decl) = &program.module {
            file_modules.insert(
                prelude_name.clone(),
                Rc::from(module_decl.path.text.as_str()),
            );
        }

        let mut globals = Vec::new();
        collect_declarations(
            program,
            &prelude_name,
            all_functions,
            all_structs,
            all_interfaces,
            all_enums,
            all_extends,
            &mut globals,
        );
    }
    Ok(())
}
