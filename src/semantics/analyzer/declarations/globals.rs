//! Top-level variable registration: type-checks each initializer in declaration order against the
//! globals/functions/types registered so far and records the resolved type for codegen.

use super::*;

impl<'a> Analyzer<'a> {
    /// Pass: analyze and register every top-level variable. Each initializer is type-checked in
    /// declaration order against the globals declared so far (forward references to later globals
    /// are not allowed) plus all already-registered functions/types. The resolved type is recorded
    /// in the module-global symbol scope so function bodies can resolve the variable, and surfaced
    /// to codegen via [`super::super::GlobalSymbol`].
    pub(in crate::semantics::analyzer) fn register_globals(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        // A synthetic, parameterless, non-async "module init" supplies the parent-function context
        // that expression analysis requires; with no `this` parameter it is treated as outside any
        // type, so initializers cannot reach private members.
        let empty_body: &'a [crate::syntax::nodes::StatementNode<'a>] = &[];
        let init_fn = FunctionNode::new(
            Vec::new(),
            synthetic_token(TokenKind::IdentifierToken, "__module_init"),
            None,
            None,
            Vec::new(),
            empty_body,
            false,
        );

        for global in node.globals.iter() {
            diagnostics.file_path = file_path_string(&global.file_path);
            self.check_reserved_name(&global.name, "variable", diagnostics);

            if global.is_public && global.is_static {
                diagnostics.report_error(
                    format!(
                        "Top-level variable '{}' cannot be both 'public' and 'static': they request opposite linkage ('public' exposes it to other modules, 'static' pins it to module-internal linkage)",
                        global.name.text
                    ),
                    Some(global.name.position),
                );
            }

            if self.globals.iter().any(|g| g.name == global.name.text) {
                diagnostics.report_error(
                    format!(
                        "Top-level variable '{}' is already defined",
                        global.name.text
                    ),
                    Some(global.name.position),
                );
                continue;
            }

            let gtable = self.global_symbol_table.clone();
            self.hir_global_init_begin();
            let init_type = self
                .analyze_expression(&global.initializer, &init_fn, &gtable, diagnostics)
                .unwrap_or(Type::Void);
            self.hir_global_init_finish(&global.name.text);

            let resolved = match &global.declared_type {
                Some(declared) => {
                    let dt = declared.get_type();
                    let it = init_type.get_type();
                    let numeric = crate::syntax::nodes::types::is_numeric_primitive(&dt)
                        && crate::syntax::nodes::types::is_numeric_primitive(&it);
                    if !numeric && it != "void" && !self.type_str_assignable(&dt, &it) {
                        diagnostics.report_error(
                            format!(
                                "Top-level variable '{}' is declared '{}' but initialized with '{}'",
                                global.name.text, dt, it
                            ),
                            Some(global.name.position),
                        );
                    }
                    declared.clone()
                }
                None => init_type,
            };

            {
                let mut table = self.global_symbol_table.borrow_mut();
                let _ = table.add_symbol(global.name.text.clone(), resolved.clone());
                if global.is_const {
                    table.mark_const(global.name.text.clone());
                }
            }

            self.globals.push(GlobalSymbol {
                name: global.name.text.clone(),
                type_str: resolved.get_type(),
                is_const: global.is_const,
                is_public: global.is_public,
                is_static: global.is_static,
                file_path: global.file_path.clone(),
            });
            // Register the HIR slot now (in declaration order) so a subsequent global's initializer
            // can resolve this one as a `Binding::Global`.
            self.hir_register_global(&global.name.text, &resolved.get_type(), global.is_const);
        }
    }
}
