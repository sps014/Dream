//! Source loading: recursive import resolution, file I/O, and merging every parsed file's
//! declarations into a single [`ProgramAccumulator`]. The merged program is what semantic
//! analysis and codegen run over.

use bumpalo::Bump;
use indexmap::IndexSet;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Error, ErrorKind, Read};
use std::path::Path;
use std::rc::Rc;

use dream_diagnostics::DiagnosticBag;
use dream_stdlib::std_package_from_slash_path;
use dream_syntax::lexer::Lexer;
use dream_syntax::nodes::struct_node::StructDeclarationNode;
use dream_syntax::nodes::{
    EnumDeclarationNode, ExtendNode, FunctionNode, GlobalVariableNode, InterfaceDeclarationNode,
    ProgramNode,
};
use dream_syntax::parser::Parser;
use dream_syntax::token::syntax_token::SyntaxToken;

/// Collects every top-level declaration from all parsed files (user code + imports + prelude +
/// `@json` derives), tagged with its originating file so semantic diagnostics attribute errors
/// correctly.
#[derive(Default)]
pub struct ProgramAccumulator<'a> {
    pub visited: HashSet<String>,
    pub all_functions: Vec<FunctionNode<'a>>,
    pub all_structs: Vec<StructDeclarationNode<'a>>,
    pub all_interfaces: Vec<InterfaceDeclarationNode<'a>>,
    pub all_enums: Vec<EnumDeclarationNode<'a>>,
    pub all_extends: Vec<ExtendNode<'a>>,
    pub all_globals: Vec<GlobalVariableNode<'a>>,
    pub file_contents: HashMap<String, String>,
    /// Every file that declared a `module a.b.c;`, mapped to its dot-joined module path. Files
    /// absent from this map belong to the implicit, unnamed root module. Handed to the analyzer
    /// (see `Analyzer::with_file_modules`) so module-scoped `internal` visibility and aliased
    /// `import ... as` resolution can compare/resolve modules after every file's individual
    /// `ProgramNode` has been flattened into one merged program.
    pub file_modules: HashMap<String, Rc<str>>,
    /// Every aliased `import a.b.c as x;` encountered across all files, as
    /// `(module path "a.b", item name "c", alias token "x")`. Resolved by the analyzer in a second
    /// pass, after every file (and its `module` declaration) is loaded, since the referenced module
    /// may be declared by a file loaded later in the recursive walk.
    pub aliased_imports: Vec<(String, String, SyntaxToken, String)>,
    /// Dotted stdlib package names requested via plain `import system.net;` (etc.). Fed to
    /// selective prelude merge together with bootstrap packages.
    pub requested_std_packages: IndexSet<String>,
}

/// Resolves an `import a.b.c;` reference (passed here as the slash-joined path `a/b/c`) relative to
/// `base_dir`, defaulting the extension to `.dream` when none is given. Falls back to a
/// `dream_packages/<pkg>/src/...` dependency directory — installed by the `dreamer` package
/// manager (`tooling/dreamer`) from that project's `dream.toml` — when no matching file exists
/// locally, so `import json_tools.parse;` finds `<project-root>/dream_packages/json_tools/src/
/// parse.dream` after `dreamer install`. Resolution here only inspects the filesystem layout
/// `dreamer` produces; it never reads `dream.toml` itself, keeping the compiler's import
/// resolution independent of the package manager's manifest format.
pub fn resolve_import_path(base_dir: &Path, module_name: &str) -> std::path::PathBuf {
    let mut import_path = base_dir.join(module_name);
    if import_path.extension().is_none() {
        import_path.set_extension("dream");
    }
    if import_path.exists() {
        return import_path;
    }
    resolve_package_import(base_dir, module_name).unwrap_or(import_path)
}

/// Resolves `module_name` (`<pkg>` or `<pkg>/<rest>`) against the nearest ancestor
/// `dream_packages/` directory. A bare `import pkg;` looks for `dream_packages/pkg/src/pkg.dream`
/// (a package's self-named entry file, mirroring the top-level convention of a file's logical
/// entry sharing its own name); `import pkg.rest;` looks for `dream_packages/pkg/src/rest.dream`.
fn resolve_package_import(base_dir: &Path, module_name: &str) -> Option<std::path::PathBuf> {
    let mut parts = module_name.splitn(2, '/');
    let package_name = parts.next().filter(|s| !s.is_empty())?;
    let rest = parts.next();

    let packages_dir = find_dream_packages_dir(base_dir)?;
    let package_src = packages_dir.join(package_name).join("src");

    let mut candidate = match rest {
        Some(rest) => package_src.join(rest),
        None => package_src.join(package_name),
    };
    if candidate.extension().is_none() {
        candidate.set_extension("dream");
    }
    candidate.exists().then_some(candidate)
}

/// Walks upward from `start_dir` looking for a `dream_packages/` directory, stopping (without a
/// match) at the first `dream.toml` project root that has none yet — e.g. before the first
/// `dreamer install` — so resolution never wanders into an unrelated ancestor project's packages.
fn find_dream_packages_dir(start_dir: &Path) -> Option<std::path::PathBuf> {
    let mut dir = Some(start_dir.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join("dream_packages");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if d.join("dream.toml").is_file() {
            return None;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// Clones every top-level declaration of `program` into the accumulators, tagging each with
/// `file_tag` so semantic diagnostics can be attributed to the right source file. Shared by the
/// recursive loader, the prelude merge, and the LSP front-end so the tagging logic never drifts.
// The many parameters are parallel per-declaration-kind accumulators; grouping them into a struct
// would just move the same field list elsewhere without improving call sites.
#[allow(clippy::too_many_arguments)]
pub fn collect_declarations<'a>(
    program: &ProgramNode<'a>,
    file_tag: &str,
    all_functions: &mut Vec<FunctionNode<'a>>,
    all_structs: &mut Vec<StructDeclarationNode<'a>>,
    all_interfaces: &mut Vec<InterfaceDeclarationNode<'a>>,
    all_enums: &mut Vec<EnumDeclarationNode<'a>>,
    all_extends: &mut Vec<ExtendNode<'a>>,
    all_globals: &mut Vec<GlobalVariableNode<'a>>,
) {
    let tag: Rc<str> = Rc::from(file_tag);

    for function in program.functions.iter().cloned() {
        let mut function = function;
        function.file_path = Some(tag.clone());
        all_functions.push(function);
    }
    for struct_decl in program.structs.iter().cloned() {
        let mut struct_decl = struct_decl;
        struct_decl.file_path = Some(tag.clone());
        for method in struct_decl.methods.iter_mut() {
            method.file_path = Some(tag.clone());
        }
        all_structs.push(struct_decl);
    }
    for interface_decl in program.interfaces.iter().cloned() {
        let mut interface_decl = interface_decl;
        interface_decl.file_path = Some(tag.clone());
        for method in interface_decl.methods.iter_mut() {
            method.file_path = Some(tag.clone());
        }
        all_interfaces.push(interface_decl);
    }
    for enum_decl in program.enums.iter().cloned() {
        let mut enum_decl = enum_decl;
        enum_decl.file_path = Some(tag.clone());
        all_enums.push(enum_decl);
    }
    for extend_decl in program.extends.iter().cloned() {
        let mut extend_decl = extend_decl;
        extend_decl.file_path = Some(tag.clone());
        for method in extend_decl.methods.iter_mut() {
            method.file_path = Some(tag.clone());
        }
        all_extends.push(extend_decl);
    }
    for global in program.globals.iter().cloned() {
        let mut global = global;
        global.file_path = Some(tag.clone());
        all_globals.push(global);
    }
}

/// Recursively parses `file_path` and every file it imports, merging all declarations into the
/// `acc` accumulators. Each declaration is tagged with its originating file so semantic
/// diagnostics (which run on the merged program) can attribute errors correctly.
pub fn parse_file_recursive<'a>(
    file_path: &String,
    acc: &mut ProgramAccumulator<'a>,
    arena: &'a Bump,
    diagnostics: &mut DiagnosticBag,
) -> Result<(), Error> {
    let path = Path::new(file_path).canonicalize()?;
    let path_str = path
        .to_str()
        .ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("Non-UTF-8 file path: {:?}", path),
            )
        })?
        .to_string();

    if acc.visited.contains(&path_str) {
        return Ok(()); // Already processed
    }
    acc.visited.insert(path_str.clone());

    let mut file = File::open(&path)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;

    // `print` (along with `to_string`/`hash_code`) is now a compiler builtin resolved during
    // code generation via the object protocol, so no source injection is needed.

    acc.file_contents.insert(path_str.clone(), text.clone());

    let mut file_diagnostics = DiagnosticBag::new(Some(path_str.clone()));

    let lexer = Lexer::new(text);
    let mut parser = Parser::new(lexer, arena, &mut file_diagnostics);

    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(e) => {
            diagnostics.extend(&file_diagnostics);
            return Err(e);
        }
    };

    diagnostics.extend(&file_diagnostics);

    let program = ast.get_root();
    if let Some(module_decl) = &program.module {
        acc.file_modules
            .insert(path_str.clone(), Rc::from(module_decl.path.text.as_str()));
    }
    let parent_dir = path.parent().unwrap_or_else(|| Path::new(""));

    for import in &program.imports {
        if let Some(alias) = &import.alias {
            // `import a.b.c as x;` names an item `c` inside a *declared* module `a.b`, not a file
            // path — resolved separately (`resolve_aliased_imports`) once every file, and its own
            // `module` declaration, has been loaded.
            let dotted = import.module_name.text.as_str();
            match dotted.rsplit_once('.') {
                Some((module_path, item)) => {
                    acc.aliased_imports.push((
                        module_path.to_string(),
                        item.to_string(),
                        alias.clone(),
                        path_str.clone(),
                    ));
                }
                None => {
                    diagnostics.report_error(
                        format!(
                            "'import {} as {}': expected a module-qualified path like 'a.b.item'",
                            dotted, alias.text
                        ),
                        Some(import.module_name.position),
                    );
                }
            }
            continue;
        }
        let module_name = import.module_name.text.as_str();
        // Reserved `system` / `system.*` packages resolve to the embedded stdlib, not the filesystem.
        if let Some(pkg) = std_package_from_slash_path(module_name) {
            acc.requested_std_packages.insert(pkg.name.to_string());
            continue;
        }
        let import_path = resolve_import_path(parent_dir, module_name);

        let import_path_str = match import_path.to_str() {
            Some(s) => s.to_string(),
            None => {
                diagnostics.report_error(
                    format!("Non-UTF-8 import path: {:?}", import_path),
                    Some(import.module_name.position),
                );
                continue;
            }
        };
        if !import_path.exists() {
            diagnostics.report_error(
                format!("Imported file not found: {}", import_path_str),
                Some(import.module_name.position),
            );
            continue;
        }

        parse_file_recursive(&import_path_str, acc, arena, diagnostics)?;
    }

    // Tag every declaration with its source file so semantic diagnostics (which run on the
    // merged program) can report the correct file name.
    collect_declarations(
        program,
        &path_str,
        &mut acc.all_functions,
        &mut acc.all_structs,
        &mut acc.all_interfaces,
        &mut acc.all_enums,
        &mut acc.all_extends,
        &mut acc.all_globals,
    );

    Ok(())
}
