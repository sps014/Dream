//! Resolves every aliased `import a.b.c as x;` collected across all files (see
//! `driver::source_loader::ProgramAccumulator::aliased_imports`) against the function table. Must
//! run after `register_functions` so cross-module duplicate-name collisions have already been
//! promoted to their module-qualified keys, but before function bodies are analyzed so an alias is
//! available to every call site that uses it.

use super::*;
use crate::syntax::token::syntax_token::SyntaxToken;

impl<'a> Analyzer<'a> {
    /// Pass: bind every collected `import a.b.c as x;` into the (file-flattened) top-level scope
    /// under its alias, resolving `a.b` as a declared module and `c` as an item inside it. Reports
    /// a diagnostic for an unknown module/item, an alias that collides with an existing name, or an
    /// item that is not visible outside its own file (private items can never be aliased in).
    pub(in crate::semantics::analyzer) fn register_import_aliases(
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
        let module: Rc<str> = Rc::from(module_path);

        // The resolved emitted key backing `module_path.item`: either a module-qualified key (a
        // cross-module collision forced a promotion) or, in the common case where `item` is
        // unique across the whole program, its own bare declaration — as long as it actually
        // belongs to the requested module and not some other (or the unnamed root) module.
        let resolved_key = self
            .function_table
            .resolve_in_module(Some(&module), item)
            .map(|s| s.to_string())
            .or_else(|| {
                self.function_table
                    .get_function(&item.to_string())
                    .ok()
                    .filter(|info| info.declaring_module.as_deref() == Some(module_path))
                    .map(|info| info.name)
            });

        let Some(key) = resolved_key else {
            diagnostics.report_error(
                format!(
                    "no item '{}' found in module '{}' (the declaring file must be reachable via a plain 'import' elsewhere in the program)",
                    item, module_path
                ),
                Some(alias.position),
            );
            return;
        };

        let info = match self.function_table.get_function(&key) {
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
        if info.visibility == crate::syntax::nodes::Visibility::Private {
            diagnostics.report_error(
                format!(
                    "'{}' in module '{}' is private; only 'public'/'internal' items can be imported with 'as'",
                    item, module_path
                ),
                Some(alias.position),
            );
            return;
        }

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

        if let Err(e) = self.function_table.add_function(alias.text.clone(), info) {
            diagnostics.report_error(e.to_string(), Some(alias.position));
        }
    }
}
