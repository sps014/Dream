//! Struct-field resolution shared by member reads/writes, member-read analysis (`obj.member`), and
//! the struct field-index lookup.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::hir::HExpr;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::FunctionTableInfo;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::mangle_generic;
use crate::syntax::nodes::{ExpressionNode, FunctionNode, ParameterNode, StatementNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::types::{method_fn, DefKind};
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

        let struct_file = self
            .struct_table
            .get_struct(&struct_name)
            .and_then(|info| info.file_path.clone());
        let field = match self.struct_table.get_struct(&struct_name) {
            Some(info) => info
                .fields
                .get(&member.text)
                .map(|f| (f.type_.clone(), f.visibility)),
            None => return MemberField::StructNotFound { struct_name },
        };

        let (field_type, field_visibility) = match field {
            Some(f) => f,
            None => return MemberField::NotAField { struct_name },
        };

        // Private fields (the default) may only be accessed from within the declaring type's own
        // methods; `internal` from anywhere in the same module; `public` exposes them everywhere.
        if !self.member_accessible(
            field_visibility,
            &struct_file,
            parent_function.file_path.as_ref(),
            self.in_methods_of(parent_function, &base_name),
        ) {
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
                let getter = method_fn(&id.text, &getter_member_name(&member.text));
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
                // Static method group `Type.method` (no call): a bound reference to a static
                // method, with no receiver to capture — identical in shape to a bare function
                // value (see `Analyzer::hir_set_func_value`), just looked up under its mangled
                // `{Type}_{method}` name instead of its own bare source name.
                if let Some(func_ty) = self.resolve_static_method_group_value(&id.text, member) {
                    return Ok(func_ty);
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
                } else if let Some(func_ty) = self.resolve_method_group_value(
                    &struct_name,
                    member,
                    &obj_type,
                    obj_hir,
                    parent_function,
                ) {
                    Ok(func_ty)
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

    /// Resolves `Type.method` (no call) as a static "method group" value: a bound reference to a
    /// static method, with no receiver to capture — the exact same runtime shape as a bare
    /// function value (see [`Analyzer::hir_set_func_value`]), just looked up under the method's
    /// mangled `{Type}_{method}` name rather than a free function's own bare source name. Returns
    /// `None` (leaving the caller to fall through to its usual diagnostic) when there is no such
    /// static method or it is part of an overload set (ambiguous without a call's argument types).
    fn resolve_static_method_group_value(
        &mut self,
        type_name: &str,
        member: &SyntaxToken,
    ) -> Option<Type> {
        let mangled = method_fn(type_name, &member.text);
        if self
            .function_table
            .overloads
            .get(&mangled)
            .map(Vec::len)
            .unwrap_or(0)
            > 1
        {
            return None;
        }
        let sig = self.function_table.get_function(&mangled).ok()?;
        if !sig.is_static {
            return None;
        }
        let box_ret = Self::async_return_type(sig.is_async, sig.return_type.clone());
        let func_ty = Type::Function(sig.parameter_types.clone(), Box::new(box_ret.clone()));
        self.hir_set_func_value(&mangled, &func_ty, &box_ret);
        Some(func_ty)
    }

    /// Resolves `receiver.method` (no call) as an instance "method group" value: a bound
    /// reference to `struct_name`'s instance method `member.text`, usable as a first-class
    /// `fun(...)` value (e.g. `WebWorker(counter.increment)`) exactly like `() =>
    /// counter.increment()` today. Lowers to the same `[funcidx, env]` closure-box shape a
    /// capturing lambda produces (see `expressions::lambda`): `receiver_hir`'s already-analyzed
    /// value is snapshotted into a fresh `CaptureCell<T>` (permanently retained, mirroring a real
    /// capture) and a synthesized lifted function `__method_group_<n>` — whose body is the
    /// ordinary call `<captured receiver>.method(args)` — reads it back apart at its own prologue
    /// (`Analyzer::hir_begin_function`), by way of the same `closure_captures`/`pending_lambdas`
    /// bookkeeping a real lambda literal uses. Returns `None` (falling through to the caller's
    /// existing "not a field" diagnostic) when there is no such method, it is static or part of an
    /// overload set (ambiguous without a call's argument types), or the receiver could not be
    /// analyzed.
    fn resolve_method_group_value(
        &mut self,
        struct_name: &str,
        member: &SyntaxToken,
        receiver_ty: &Type,
        receiver_hir: Option<HExpr>,
        parent_function: &FunctionNode<'a>,
    ) -> Option<Type> {
        let mangled = method_fn(struct_name, &member.text);
        if self
            .function_table
            .overloads
            .get(&mangled)
            .map(Vec::len)
            .unwrap_or(0)
            > 1
        {
            return None;
        }
        let sig = self.function_table.get_function(&mangled).ok()?;
        if sig.is_static {
            return None;
        }
        // Privacy is enforced naturally below: the synthesized lifted function's own body calls
        // `<captured receiver>.member(args)` exactly like ordinary user source would, through the
        // same method-call resolution every other call site goes through (see
        // `calls::member_calls`), so an inaccessible method is rejected there — at `member`'s own
        // (real, user-source) position, since `member_tok` below is built from `member.clone()`
        // rather than a synthetic token.
        let receiver_hir = receiver_hir?;

        // `parameter_types[0]` is the implicit `this` (see `register_methods_for`); the value's
        // own `fun(...)` signature is everything declared after it. An async method's call yields
        // `Future<T>`, so the bound method-group value is `fun(...): Future<T>` — the synthesized
        // wrapper stays synchronous and simply returns that Future handle (same as boxing a named
        // async function as a `fun(...): Future<T>` value).
        let param_types: Vec<Type> = sig.parameter_types.iter().skip(1).cloned().collect();
        let box_ret = Self::async_return_type(sig.is_async, sig.return_type.clone());
        let func_ty = Type::Function(param_types.clone(), Box::new(box_ret.clone()));

        let name = format!("__method_group_{}", self.lambda_counter);
        self.lambda_counter += 1;

        let recv_name = "__mg_recv".to_string();
        let recv_tok = synthetic_token(TokenKind::IdentifierToken, &recv_name);
        let recv_expr: &'a ExpressionNode<'a> =
            self.arena.alloc(ExpressionNode::Identifier(recv_tok));

        let mut parameters: Vec<ParameterNode> = Vec::with_capacity(param_types.len());
        let mut arg_exprs: Vec<ExpressionNode<'a>> = Vec::with_capacity(param_types.len());
        for (i, ty) in param_types.iter().enumerate() {
            let ptok = synthetic_token(TokenKind::IdentifierToken, &format!("__mg_arg{i}"));
            parameters.push(ParameterNode::new(ptok.clone(), ty.clone()));
            arg_exprs.push(ExpressionNode::Identifier(ptok));
        }

        let member_tok = member.clone();
        let call_stmt = if matches!(box_ret, Type::Void) {
            StatementNode::MethodInvocation(recv_expr, member_tok, None, arg_exprs)
        } else {
            StatementNode::Return(Some(ExpressionNode::MethodCall(
                recv_expr, member_tok, None, arg_exprs,
            )))
        };
        let body: &'a [StatementNode<'a>] = self.arena.alloc_slice_clone(&[call_stmt]);

        let func_node = FunctionNode {
            attributes: Vec::new(),
            name: synthetic_token(TokenKind::IdentifierToken, &name),
            generic_parameters: None,
            generic_constraints: Vec::new(),
            return_type: Some(box_ret.clone()),
            parameters,
            body,
            visibility: crate::syntax::nodes::Visibility::Private,
            is_extern: false,
            is_static: false,
            is_async: false,
            file_path: parent_function.file_path.clone(),
            accessor: None,
            is_default_impl: false,
        };
        let func_ref: &'a FunctionNode<'a> = self.arena.alloc(func_node);

        let info = FunctionTableInfo::from(func_ref);
        // Synthesized names are always fresh (a monotonically increasing counter, shared with
        // `expressions::lambda`'s own `__lambda_<n>` names), so this cannot collide.
        let _ = self.function_table.add_function(name.clone(), info);
        self.type_ctx.register(DefKind::Function, &name, vec![]);
        self.pending_lambdas
            .insert(name.clone(), (func_ref, self.current_generic_bindings.clone()));
        self.closure_captures
            .insert(name.clone(), vec![(recv_name, receiver_ty.clone())]);

        let cell = self.hir_build_cell_new(receiver_ty, receiver_hir)?;
        self.hir_retain_env(cell.clone());
        self.hir_set_capturing_func_value(&name, cell, &func_ty, &box_ret);
        Some(func_ty)
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
