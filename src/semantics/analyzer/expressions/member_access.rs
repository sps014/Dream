//! Struct-field resolution shared by member reads/writes, member-read analysis (`obj.member`), and
//! the struct field-index lookup.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::types::method_fn;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    /// Resolves `member` against an already-analyzed, non-`js`, non-enum receiver of type `obj_type`
    /// as a struct field. Shared by member reads (`obj.m`) and writes (`obj.m = v`): instantiates a
    /// generic receiver on demand and, for a resolved field, reports the "private field" diagnostic
    /// when it is not accessible from `parent_function`. The non-`Field` outcomes are returned for
    /// the caller to handle, since read/write positions differ in accessor desugaring and in how they
    /// report errors.
    pub(in crate::semantics::analyzer) fn resolve_member_field(
        &mut self,
        obj_type: &Type,
        member: &SyntaxToken,
        parent_function: &FunctionNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) -> MemberField {
        let (base_name, generic_args) = match Self::resolve_struct_parts(obj_type) {
            Some(parts) => parts,
            None => return MemberField::NotAStruct,
        };

        self.ensure_struct_instantiated(&base_name, &generic_args, &member.position, diagnostics);
        let struct_name = mangle_generic(&base_name, &generic_args);

        let field = match self.struct_table.get_struct(&struct_name) {
            Some(info) => info
                .fields
                .get(&member.text)
                .map(|f| (f.type_.clone(), f.is_public)),
            None => return MemberField::StructNotFound { struct_name },
        };

        let (field_type, field_is_public) = match field {
            Some(f) => f,
            None => return MemberField::NotAField { struct_name },
        };

        // Private fields (the default) may only be accessed from within the declaring type's own
        // methods; `public` exposes them to outside code.
        if !field_is_public && !self.in_methods_of(parent_function, &base_name) {
            diagnostics.report_error(
                format!("'{}' is private to '{}'", member.text, base_name),
                Some(member.position),
            );
        }

        MemberField::Field {
            struct_name,
            field_type,
        }
    }

    /// Types a member access `obj.member`: discriminated-union unit-variant construction
    /// (`Option.None`), enum member access (`Color.Red`), and struct field access (with generic
    /// instantiation and field-privacy enforcement). Returns the accessed field/member type.
    pub(super) fn analyze_member_access(
        &mut self,
        obj: &'a ExpressionNode<'a>,
        member: &SyntaxToken,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        // A unit variant of a discriminated union (`Shape.Empty`, `Option.None`) constructs
        // a heap union value rather than resolving to an integer enum member.
        if let ExpressionNode::Identifier(id) = obj {
            if let Some(t) = self.analyze_variant_construction(
                &id.text,
                member,
                &[],
                parent_function,
                symbol_table,
                diagnostics,
            )? {
                // `analyze_variant_construction` records the `UnionNew` (or clears `last`) itself.
                return Ok(t);
            }
        }
        // Enum member access `EnumName.Member` resolves to the enum type (an i32 at runtime).
        if let ExpressionNode::Identifier(id) = obj {
            if self.enum_table.contains_key(&id.text) {
                let enum_ty = Type::Struct(id.clone(), None);
                match self.enum_member_value(&id.text, &member.text) {
                    Some(value) => self.hir_set_enum_value(value as i64, &enum_ty),
                    None => {
                        diagnostics.report_error(
                            format!("Enum '{}' has no member '{}'", id.text, member.text),
                            Some(member.position),
                        );
                        self.hir_none();
                    }
                }
                return Ok(enum_ty);
            }
        }
        // `js.global` as a value (not the `js.global("name")` call form) is `globalThis`, so
        // `js.global.document` / `js.global.fetch(...)` chain naturally off the JS global scope.
        if let ExpressionNode::Identifier(id) = obj {
            let is_local = symbol_table.borrow().get_symbol(id).is_ok();
            if !is_local && id.text == "js" && member.text == "global" {
                self.desugar_js_global_this();
                return Ok(Self::js_type());
            }
        }
        // Static property getter `Type.prop`: when the receiver names a type (not a local) and a
        // static getter exists, desugar to a static call `Type.get$prop()` (mirrors the instance
        // getter desugar below, but the receiver is the type rather than a value).
        if let ExpressionNode::Identifier(id) = obj {
            let is_local = symbol_table.borrow().get_symbol(id).is_ok();
            if !is_local {
                let type_name = crate::syntax::nodes::types::canonical_type_name(&id.text)
                    .unwrap_or(id.text.as_str())
                    .to_string();
                let getter = method_fn(&type_name, &getter_member_name(&member.text));
                if self.function_table.get_function(&getter).is_ok() {
                    let get_tok = synthetic_token(
                        TokenKind::IdentifierToken,
                        &getter_member_name(&member.text),
                    );
                    let call = ExpressionNode::MethodCall(obj, get_tok, None, vec![]);
                    return self.analyze_expression(
                        &call,
                        parent_function,
                        symbol_table,
                        diagnostics,
                    );
                }
            }
        }

        let obj_type = self.analyze_expression(obj, parent_function, symbol_table, diagnostics)?;
        let obj_hir = self.hir_take();

        // The receiver was already poisoned by an earlier error: stay quiet and stay poison.
        if obj_type.is_unknown() {
            self.hir_none();
            return Ok(Type::Unknown);
        }

        // A `js`-typed receiver has no static fields: `obj.name` reads a JS property dynamically.
        if self.is_js_type(&obj_type) {
            self.desugar_js_get(obj_hir, &member.text);
            return Ok(Self::js_type());
        }

        match self.resolve_member_field(&obj_type, member, parent_function, diagnostics) {
            MemberField::Field {
                struct_name,
                field_type,
            } => {
                match self.struct_field_index(&struct_name, &member.text) {
                    Some(index) => self.hir_set_field(obj_hir, index, &field_type),
                    None => self.hir_none(),
                }
                Ok(field_type)
            }
            MemberField::NotAStruct => {
                self.hir_none();
                Err(report(
                    diagnostics,
                    format!(
                        "Cannot access member of non-class type {}",
                        obj_type.get_type()
                    ),
                    Some(member.position),
                ))
            }
            MemberField::StructNotFound { struct_name } => {
                self.hir_none();
                Err(report(
                    diagnostics,
                    format!("Struct '{}' not found", struct_name),
                    Some(member.position),
                ))
            }
            MemberField::NotAField { struct_name } => {
                // Not a field: `obj.prop` may read a property getter, which desugars to a call of
                // the (internally named) getter method. The call carries its own privacy/type check.
                let getter = method_fn(&struct_name, &getter_member_name(&member.text));
                if self.function_table.get_function(&getter).is_ok() {
                    let get_tok = synthetic_token(
                        TokenKind::IdentifierToken,
                        &getter_member_name(&member.text),
                    );
                    let call = ExpressionNode::MethodCall(obj, get_tok, None, vec![]);
                    self.analyze_expression(&call, parent_function, symbol_table, diagnostics)
                } else {
                    self.hir_none();
                    Err(report(
                        diagnostics,
                        format!(
                            "Field '{}' not found in class '{}'",
                            member.text, struct_name
                        ),
                        Some(member.position),
                    ))
                }
            }
        }
    }

    /// Resolves a field's position in a struct's layout (offset order, matching the
    /// auto-generated constructor's argument order and the backend's field indexing). Returns
    /// `None` if the struct or field is unknown.
    pub(in crate::semantics::analyzer) fn struct_field_index(
        &self,
        struct_name: &str,
        field: &str,
    ) -> Option<usize> {
        let info = self.struct_table.get_struct(struct_name)?;
        let mut ordered: Vec<(&String, &crate::semantics::struct_table::StructFieldInfo)> =
            info.fields.iter().collect();
        ordered.sort_by_key(|(_, f)| f.offset);
        ordered.iter().position(|(n, _)| n.as_str() == field)
    }
}
