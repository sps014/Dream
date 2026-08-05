//! Resolves every aliased `import a.b.c as x;` collected across all files (see
//! `driver::source_loader::ProgramAccumulator::aliased_imports`) against the function table. Must
//! run after `register_functions` so cross-module duplicate-name collisions have already been
//! promoted to their module-qualified namespaces, but before function bodies are analyzed so an
//! alias is available to every call site that uses it.

use super::*;
use dream_syntax::nodes::Visibility;
use dream_syntax::token::syntax_token::SyntaxToken;

impl<'a> Analyzer<'a> {
    /// Pass: bind every collected `import a.b.c as x;` into the (file-flattened) top-level scope
    /// under its alias, resolving `a.b` as a declared module and `c` as an item inside it. Reports
    /// a diagnostic for an unknown module/item, an alias that collides with an existing name, or an
    /// item that is not visible outside its own file (private items can never be aliased in).
    pub(in crate::analyzer) fn register_import_aliases(
        &mut self,
        diagnostics: &mut DiagnosticBag,
    ) {
        let aliased = std::mem::take(&mut self.aliased_imports);
        for (module_path, item, alias, importing_file) in aliased {
            diagnostics.file_path = Some(importing_file);
            self.register_one_import_alias(&module_path, &item, &alias, diagnostics);
        }
    }

    fn register_one_import_alias(
        &mut self,
        module_path: &str,
        item: &str,
        alias: &SyntaxToken,
        diagnostics: &mut DiagnosticBag,
    ) {
        // Overload namespace for `module_path.item`: module-qualified after a cross-module
        // collision, otherwise the bare name when that namespace belongs to the requested module.
        let Some(ns) = self
            .function_table
            .resolve_item_namespace(module_path, item)
        else {
            diagnostics.report_error(
                format!(
                    "no item '{}' found in module '{}' (the declaring file must be reachable via a plain 'import' elsewhere in the program)",
                    item, module_path
                ),
                Some(alias.position),
            );
            return;
        };

        if self.function_table.get_function(&alias.text).is_ok()
            || self.function_table.is_overloaded(&alias.text)
        {
            diagnostics.report_error(
                format!(
                    "cannot import '{}.{}' as '{}': '{}' is already defined",
                    module_path, item, alias.text, alias.text
                ),
                Some(alias.position),
            );
            return;
        }

        if self.function_table.is_overloaded(&ns) {
            self.bind_overloaded_import_alias(module_path, item, &ns, alias, diagnostics);
            return;
        }

        let info = match self.function_table.get_function(&ns) {
            Ok(info) => info,
            Err(_) => {
                diagnostics.report_error(
                    format!("no item '{}' found in module '{}'", item, module_path),
                    Some(alias.position),
                );
                return;
            }
        };

        // A private item is file-scoped and can never be aliased in from another file (unlike
        // `internal`, which this pass conservatively allows: the alias mechanism does not track the
        // importing file separately from every other file in the program, so it cannot check "same
        // module as the importer" precisely — see docs/language/imports.md).
        if info.visibility == Visibility::Private {
            diagnostics.report_error(
                format!(
                    "'{}' in module '{}' is private; only 'public'/'internal' items can be imported with 'as'",
                    item, module_path
                ),
                Some(alias.position),
            );
            return;
        }

        // Keep `info.name` as the original emitted key so calls through the alias target the
        // real WASM symbol / DefId; only the table lookup key is the alias.
        if let Err(e) = self.function_table.add_function(alias.text.clone(), info) {
            diagnostics.report_error(e.to_string(), Some(alias.position));
        }
    }

    /// Binds an overloaded module item under `alias` by pointing `overloads[alias]` at the same
    /// emitted keys as the source namespace — call sites resolve overloads against the alias name
    /// but still call the original DefIds / WASM symbols.
    fn bind_overloaded_import_alias(
        &mut self,
        module_path: &str,
        item: &str,
        ns: &str,
        alias: &SyntaxToken,
        diagnostics: &mut DiagnosticBag,
    ) {
        let keys = match self.function_table.overloads.get(ns) {
            Some(keys) => keys.clone(),
            None => {
                diagnostics.report_error(
                    format!("no item '{}' found in module '{}'", item, module_path),
                    Some(alias.position),
                );
                return;
            }
        };

        for key in &keys {
            let info = match self.function_table.get_function(key) {
                Ok(info) => info,
                Err(_) => {
                    diagnostics.report_error(
                        format!("no item '{}' found in module '{}'", item, module_path),
                        Some(alias.position),
                    );
                    return;
                }
            };
            if info.visibility == Visibility::Private {
                diagnostics.report_error(
                    format!(
                        "'{}' in module '{}' is private; only 'public'/'internal' items can be imported with 'as'",
                        item, module_path
                    ),
                    Some(alias.position),
                );
                return;
            }
        }

        self.function_table
            .overloads
            .insert(alias.text.clone(), keys);
    }
}
