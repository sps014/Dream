//! Standard-library prelude merging. Each built-in type lives in its own embedded prelude file
//! (`crate::stdlib::PRELUDE_FILES`); their declarations are parsed with the user's arena and
//! merged into the program so the built-in types are real, extensible classes.

use bumpalo::Bump;
use std::collections::HashMap;
use std::io::Error;

use crate::diagnostics::DiagnosticBag;
use crate::driver::source_loader::collect_declarations;
use crate::syntax::lexer::Lexer;
use crate::syntax::parser::Parser;

/// Parses the embedded standard-collections prelude and merges its declarations into the
/// program. Uses the same arena as the user's files so all AST nodes share a lifetime.
// The many parameters are parallel per-declaration-kind accumulators; grouping them into a struct
// would just move the same field list elsewhere without improving call sites.
#[allow(clippy::too_many_arguments)]
pub fn merge_prelude<'a>(
    arena: &'a Bump,
    all_functions: &mut Vec<crate::syntax::nodes::FunctionNode<'a>>,
    all_structs: &mut Vec<crate::syntax::nodes::struct_node::StructDeclarationNode<'a>>,
    all_interfaces: &mut Vec<crate::syntax::nodes::InterfaceDeclarationNode<'a>>,
    all_enums: &mut Vec<crate::syntax::nodes::EnumDeclarationNode>,
    all_extends: &mut Vec<crate::syntax::nodes::ExtendNode<'a>>,
    diagnostics: &mut DiagnosticBag,
    file_contents: &mut HashMap<String, String>,
) -> Result<(), Error> {
    // Each standard type lives in its own prelude file. The primitive files (int/char/string/...)
    // make the built-in types real, extensible classes via `extend` blocks. The list itself lives
    // in `crate::stdlib::PRELUDE_FILES` so the analyzer language service shares the same manifest.
    for &(prelude_name, prelude_src) in crate::stdlib::PRELUDE_FILES {
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

        // Preludes declare no globals; a throwaway sink keeps the shared collector signature.
        let mut globals = Vec::new();
        collect_declarations(
            ast.get_root(),
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
