//! Type-checking `Enum.Variant(args)` / unit `Enum.Variant` construction, for both concrete unions
//! and generic ones (where the concrete type arguments must be resolved from an expected type or
//! inferred from the constructor arguments before the instance can be monomorphized).

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// If `enum_name` denotes a discriminated union (concrete or generic) and `variant` names one
    /// of its variants, type-checks the construction `Enum.Variant(args)` and returns its type.
    /// Returns `Ok(None)` when `enum_name` is not a union (so the caller can fall through to its
    /// normal handling, e.g. C-style enum member access or a static method call).
    pub(in crate::semantics::analyzer) fn analyze_variant_construction(
        &mut self,
        enum_name: &str,
        variant: &SyntaxToken,
        args: &[ExpressionNode<'a>],
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Option<Type>, SemanticError> {
        let is_generic = self.generic_unions.contains_key(enum_name);
        let is_concrete = self.union_table.contains_key(enum_name);
        if !is_generic && !is_concrete {
            return Ok(None);
        }

        // File/module-level visibility (Axis 2): a non-public enum is only referenceable from its
        // declaring file.
        self.check_type_visible(
            enum_name,
            parent_function.file_path.as_ref(),
            variant.position,
            diagnostics,
        );

        // The declared payload field types of the named variant (templated for generic unions).
        let field_types: Vec<Type> = if let Some(&template) = self.generic_unions.get(enum_name) {
            match template
                .variants
                .iter()
                .find(|v| v.name.text == variant.text)
            {
                Some(v) => v.fields.iter().map(|f| f.field_type.clone()).collect(),
                None => {
                    return Err(report(
                        diagnostics,
                        format!("Enum '{}' has no variant '{}'", enum_name, variant.text),
                        Some(variant.position),
                    ));
                }
            }
        } else {
            let info = match self.union_table.get(enum_name) {
                Some(info) => info,
                None => {
                    return Err(report(
                        diagnostics,
                        format!("Enum '{}' could not be resolved", enum_name),
                        Some(variant.position),
                    ));
                }
            };
            match info.variant(&variant.text) {
                Some(v) => v.fields.iter().map(|f| f.type_.clone()).collect(),
                None => {
                    return Err(report(
                        diagnostics,
                        format!("Enum '{}' has no variant '{}'", enum_name, variant.text),
                        Some(variant.position),
                    ));
                }
            }
        };

        let mut arg_types = Vec::new();
        let mut arg_hirs = Vec::new();
        for arg in args {
            let t = self.analyze_expression(arg, parent_function, symbol_table, diagnostics)?;
            arg_hirs.push(self.hir_take());
            arg_types.push(t);
        }

        if !is_generic {
            self.validate_variant_payload(
                enum_name,
                &variant.text,
                &field_types,
                &arg_types,
                variant.position,
                diagnostics,
            );
            let result_ty =
                Type::Struct(synthetic_token(TokenKind::IdentifierToken, enum_name), None);
            // Construct the union value: resolve its `DefId` and the variant's discriminant.
            let def = self
                .type_ctx
                .defs
                .lookup(crate::types::DefKind::Union, enum_name);
            let disc = self
                .union_table
                .get(enum_name)
                .and_then(|i| i.variant(&variant.text))
                .map(|v| v.discriminant as usize);
            match (def, disc) {
                (Some(def), Some(disc)) => self.hir_set_union_new(def, disc, arg_hirs, &result_ty),
                _ => self.hir_none(),
            }
            return Ok(Some(result_ty));
        }

        // Generic union: resolve the concrete type arguments, preferring an explicit expected type
        // (e.g. a `let`/`return` annotation) and otherwise inferring from the arguments.
        let template = *self.generic_unions.get(enum_name).unwrap_or_else(|| {
            crate::internal_error!(
                "generic union '{}' reached generic-instantiation analysis without a registered template",
                enum_name
            )
        });
        let params: Vec<String> = template
            .generic_parameters
            .as_ref()
            .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
            .unwrap_or_default();

        let mut concrete_args: Option<Vec<Type>> = None;
        if let Some(Type::Struct(b, Some(eargs))) = &self.current_expected_type {
            if b.text == enum_name && eargs.len() == params.len() {
                concrete_args = Some(eargs.clone());
            }
        }
        if concrete_args.is_none() {
            let mut binding: HashMap<String, Type> = HashMap::new();
            for (ft, at) in field_types.iter().zip(arg_types.iter()) {
                let name = ft.get_type();
                if params.contains(&name) {
                    binding.entry(name).or_insert_with(|| at.clone());
                }
            }
            let resolved: Vec<Type> = params
                .iter()
                .filter_map(|p| binding.get(p).cloned())
                .collect();
            if resolved.len() == params.len() {
                concrete_args = Some(resolved);
            }
        }

        let concrete_args = match concrete_args {
            Some(a) => a,
            None => {
                return Err(report(
                    diagnostics,
                    format!(
                        "Cannot infer type arguments for '{}.{}'; add a type annotation (e.g. `let x: {}<...> = ...`)",
                        enum_name, variant.text, enum_name
                    ),
                    Some(variant.position),
                ));
            }
        };

        let bindings = generic_bindings(
            template.generic_parameters.as_deref().unwrap_or(&[]),
            &concrete_args,
        );
        let expected_fields: Vec<Type> = field_types
            .iter()
            .map(|ft| substitute_generic_type(ft, &bindings))
            .collect();
        self.validate_variant_payload(
            enum_name,
            &variant.text,
            &expected_fields,
            &arg_types,
            variant.position,
            diagnostics,
        );

        self.ensure_union_instantiated(enum_name, &concrete_args, &variant.position, diagnostics);
        // Construct the monomorphized union value. Its interned type (`union_ty(def, args)`) matches
        // the layout keyed by the mangled instance, so the backend resolves variant offsets. The
        // shared template `DefId` + the discriminant from the concrete instance name select the arm.
        let result_ty = Type::Struct(
            synthetic_token(TokenKind::IdentifierToken, enum_name),
            Some(concrete_args),
        );
        let mangled = crate::syntax::nodes::types::mangle_generic(
            enum_name,
            match &result_ty {
                Type::Struct(_, Some(a)) => a,
                _ => unreachable!(),
            },
        );
        let def = self
            .type_ctx
            .defs
            .lookup(crate::types::DefKind::Union, enum_name);
        let disc = self
            .union_table
            .get(&mangled)
            .and_then(|i| i.variant(&variant.text))
            .map(|v| v.discriminant as usize);
        match (def, disc) {
            (Some(def), Some(disc)) => self.hir_set_union_new(def, disc, arg_hirs, &result_ty),
            _ => self.hir_none(),
        }
        Ok(Some(result_ty))
    }
}
