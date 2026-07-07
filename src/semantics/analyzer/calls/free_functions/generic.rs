//! Monomorphizing a generic free function: registering one concrete instance (for a call) and
//! instantiating one for use as a first-class function value.

use super::*;
use crate::intrinsics;
use crate::semantics::function_table::FunctionTableInfo;

impl<'a> Analyzer<'a> {
    /// Registers one monomorphized instance of a generic free function under its mangled name
    /// (`swap_int_string`, `natural_order_int`, ...), idempotently: a clone with its signature made
    /// concrete is stashed in `instantiated_generics` so its body is analyzed under `bindings`, and
    /// a matching signature is added to the function table. Shared by generic call resolution and by
    /// using a generic function as a first-class value. Returns the mangled name.
    pub(crate) fn register_generic_function_instance(
        &mut self,
        template: &'a FunctionNode<'a>,
        bindings: &GenericBindings,
    ) -> String {
        let mangled_name = mangle_bindings(&template.name.text, bindings);
        if self.function_table.get_function(&mangled_name).is_err() {
            // Store a clone with its signature monomorphized (params + return type made concrete),
            // mirroring how struct methods are specialized. The body is shared and resolved against
            // the bindings during analysis/codegen, so the declared return type (e.g. `List<T>` ->
            // `List_int`) stays consistent with what the body builds.
            let mut specialized = template.clone();
            Self::substitute_generic_signature(&mut specialized, bindings);
            let specialized_ref: &'a FunctionNode<'a> = self.arena.alloc(specialized);
            self.instantiated_generics
                .insert(mangled_name.clone(), (bindings.clone(), specialized_ref));

            let info = FunctionTableInfo {
                name: mangled_name.clone(),
                parameters: template
                    .parameters
                    .iter()
                    .map(|p| Self::monomorphize_type(&p.type_, bindings).get_type())
                    .collect(),
                defaults: template
                    .parameters
                    .iter()
                    .map(|p| p.default.clone())
                    .collect(),
                return_type: template
                    .return_type
                    .as_ref()
                    .map(|ret| Self::monomorphize_type(ret, bindings)),
                is_async: template.is_async,
                is_static: template.is_static,
                is_public: template.is_public,
                intrinsic_name: intrinsics::intrinsic_key(&template.attributes),
                declaring_file: template.file_path.clone(),
            };

            let _ = self.function_table.add_function(mangled_name.clone(), info);
        }
        mangled_name
    }

    /// Instantiates a generic free function used as a first-class *value* (`let cmp: fun(T, T): int =
    /// natural_order;`). The concrete type arguments are inferred by unifying the template's declared
    /// parameter/return types with the `expected` function type at the use site; the instance is
    /// registered (see `register_generic_function_instance`) and a `FuncValue` referencing its
    /// mangled name is emitted. Returns the monomorphized function type, or `None` (with a
    /// diagnostic) if there is no function-typed context to infer from.
    pub(crate) fn instantiate_generic_function_value(
        &mut self,
        id: &SyntaxToken,
        diagnostics: &mut DiagnosticBag,
    ) -> Option<Type> {
        let template = *self.generic_functions.get(&id.text)?;

        // The expected type at this site drives inference; it must be a concrete function type.
        let expected = self
            .current_expected_type
            .as_ref()
            .map(|t| Self::monomorphize_type(t, &self.current_generic_bindings));
        let Some(Type::Function(exp_params, exp_ret)) = expected else {
            diagnostics.report_error(
                format!(
                    "generic function '{}' can only be used as a value in a context with a known function type (e.g. `let f: fun(int, int): int = {};`)",
                    id.text, id.text
                ),
                Some(id.position),
            );
            return None;
        };

        // Infer bindings by matching the expected parameter types against the template's formals,
        // then verify the type parameters' constraints are satisfied by those concrete types.
        let param_strings: Vec<String> = exp_params.iter().map(|p| p.get_type()).collect();
        let bindings =
            self.infer_generic_bindings(template, &None, &param_strings, &id.position, diagnostics);
        self.verify_generic_constraints(
            &template.generic_constraints,
            &bindings,
            &id.position,
            diagnostics,
        );

        self.register_generic_function_instance(template, &bindings);
        // The func value must reference the base template's `DefId` + concrete instance args (in
        // binding order) so it maps to the monomorphized instance's function-table slot.
        let instance: Vec<crate::types::TypeId> =
            bindings.values().map(|t| self.type_ctx.lower(t)).collect();
        let ret = (*exp_ret).clone();
        let func_ty = Type::Function(exp_params, exp_ret);
        self.hir_set_generic_func_value(&template.name.text, instance, &func_ty, &ret);
        Some(func_ty)
    }
}
