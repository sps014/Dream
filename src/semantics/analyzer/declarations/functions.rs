//! Function-signature registration (including overload/`main` validation and public-visibility
//! leakage checks) and the body-analysis / pending-instantiation fixpoint passes.

use super::*;
use crate::semantics::function_table::FunctionTableInfo;
use crate::syntax::nodes::types::{strip_array, strip_nullable};

impl<'a> Analyzer<'a> {
    /// Pass 1: register every (non-generic) function signature; stash generic templates.
    pub(in crate::semantics::analyzer) fn register_functions(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        for function in node.functions.iter() {
            diagnostics.file_path = file_path_string(&function.file_path);
            self.check_reserved_name(&function.name, "function", diagnostics);
            if function.generic_parameters.is_some() {
                self.type_ctx.register(
                    DefKind::Function,
                    &function.name.text,
                    generic_param_names(&function.generic_parameters),
                );
                self.generic_functions
                    .insert(function.name.text.clone(), function);
                continue;
            }
            if function.is_public {
                self.check_public_visibility(function, diagnostics);
            }
            let info = FunctionTableInfo::from(function);
            if let Err(e) =
                self.function_table
                    .add_overload(&function.name.text, info, &mut self.type_ctx)
            {
                diagnostics.report_error(e.to_string(), Some(function.name.position));
            }
        }
        // Register a distinct `DefId` for every non-generic function under its *emitted* name (the
        // bare base when unique, the signature-mangled key when overloaded). Deferred to here so the
        // full overload set is known: overloaded declarations must not collide on a single base def.
        for function in node.functions.iter() {
            if function.generic_parameters.is_some() {
                continue;
            }
            let param_types: Vec<String> = function
                .parameters
                .iter()
                .map(|p| p.type_.get_type())
                .collect();
            let emitted = self.function_table.resolve_emitted_name(
                &function.name.text,
                &param_types,
                &mut self.type_ctx,
            );
            self.type_ctx.register(DefKind::Function, &emitted, vec![]);
        }
        // The entry point is exported under the fixed name `main`. It may be declared as `main()`
        // or `main(args: string[])`, but not overloaded or given any other signature.
        if self.function_table.is_overloaded("main") {
            diagnostics.report_error("'main' cannot be overloaded".to_string(), None);
        } else if let Ok(info) = self.function_table.get_function(&"main".to_string()) {
            let ok = info.parameters.is_empty()
                || (info.parameters.len() == 1 && info.parameters[0] == "string[]");
            if !ok {
                diagnostics.report_error(
                    "'main' must be declared as 'main()' or 'main(args: string[])'".to_string(),
                    None,
                );
            }
        }
    }

    /// Ensures a `public` function does not leak a private (non-`public`) class through its
    /// signature, which would make the class unusable by the callers the function is exposed to.
    pub(in crate::semantics::analyzer) fn check_public_visibility(
        &self,
        function: &FunctionNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        let signature_types = function
            .return_type
            .iter()
            .chain(function.parameters.iter().map(|p| &p.type_));
        for type_to_check in signature_types {
            let base_type_str = strip_nullable(strip_array(&type_to_check.get_type())).to_string();
            if let Some(struct_info) = self.struct_table.get_struct(&base_type_str) {
                if !struct_info.is_public {
                    diagnostics.report_error(
                        format!(
                            "Public function '{}' exposes private class '{}'",
                            function.name.text, base_type_str
                        ),
                        Some(function.name.position),
                    );
                }
            }
        }
    }

    /// Pass 2: analyze the body of every concrete function.
    pub(in crate::semantics::analyzer) fn analyze_function_bodies(
        &mut self,
        node: &'a ProgramNode<'a>,
        symbol_table_map: &mut HashMap<String, Rc<RefCell<SymbolTable>>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        for function in node.functions.iter() {
            if function.generic_parameters.is_some() {
                continue;
            }
            // Extern functions have no body; their signature is enough for call-site checks.
            if function.is_extern {
                continue;
            }
            diagnostics.file_path = file_path_string(&function.file_path);
            let table = self.analyze_function(function, diagnostics)?;
            // Key the symbol table by the emitted name so overloaded functions (which share a
            // base name but emit distinct mangled names) each get their own entry, matching the
            // name codegen uses.
            let param_types: Vec<String> = function
                .parameters
                .iter()
                .map(|p| p.type_.get_type())
                .collect();
            let key = self.function_table.resolve_emitted_name(
                &function.name.text,
                &param_types,
                &mut self.type_ctx,
            );
            symbol_table_map.insert(key, table);
        }
        Ok(())
    }

    /// Passes 3 & 4 (combined fixpoint): analyze the bodies of every monomorphized generic
    /// function instance and every (de-sugared) struct method.
    ///
    /// Analyzing one body can lazily instantiate *more* generics — a struct method that uses
    /// `List<JsonValue>` queues new struct methods, and a builder that calls `List<JsonValue>()`
    /// queues a new generic function instance. The two feed each other, so we loop until neither
    /// the generic-function set nor the struct-method list grows. Both instantiation paths are
    /// idempotent (guarded by the struct/function tables), so this terminates.
    pub(in crate::semantics::analyzer) fn analyze_pending_instantiations(
        &mut self,
        symbol_table_map: &mut HashMap<String, Rc<RefCell<SymbolTable>>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let mut processed_generics: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut method_index = 0;
        loop {
            let mut progressed = false;

            // Monomorphized generic function instances (e.g. `List<JsonValue>`, `swap_int_string`).
            let pending: Vec<String> = self
                .instantiated_generics
                .keys()
                .filter(|k| !processed_generics.contains(*k))
                .cloned()
                .collect();
            for mangled_name in pending {
                processed_generics.insert(mangled_name.clone());
                let (bindings, template) = match self.instantiated_generics.get(&mangled_name) {
                    Some((b, t)) => (b.clone(), *t),
                    None => continue,
                };
                diagnostics.file_path = file_path_string(&template.file_path);
                let table = self.with_generic_bindings(bindings, |s| {
                    s.analyze_function(template, diagnostics)
                })?;
                symbol_table_map.insert(mangled_name, table);
                progressed = true;
            }

            // De-sugared struct methods, including those for newly instantiated generic structs.
            while method_index < self.struct_methods.len() {
                let (method, bindings) = self.struct_methods[method_index].clone();
                method_index += 1;
                diagnostics.file_path = file_path_string(&method.file_path);
                let table = self
                    .with_generic_bindings(bindings, |s| s.analyze_function(method, diagnostics))?;
                // Key by the emitted name so overloaded methods each get a distinct entry (the
                // parameter list includes the implicit `this`).
                let param_types: Vec<String> = method
                    .parameters
                    .iter()
                    .map(|p| p.type_.get_type())
                    .collect();
                let key = self.function_table.resolve_emitted_name(
                    &method.name.text,
                    &param_types,
                    &mut self.type_ctx,
                );
                symbol_table_map.insert(key, table);
                progressed = true;
            }

            if !progressed {
                break;
            }
        }
        Ok(())
    }
}
