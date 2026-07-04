//! Analysis of expressions: expression typing, binary operators, type compatibility checks, and
//! identifier resolution.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::syntax::nodes::types::{is_numeric_primitive, mangle_generic, strip_nullable};
use crate::syntax::nodes::{ExpressionNode, FunctionNode, Type};
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::text::text_span::TextSpan;
use crate::types::method_fn;
use std::cell::RefCell;
use std::rc::Rc;

impl<'a> Analyzer<'a> {
    pub(super) fn analyze_expression(
        &mut self,
        expression: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        match expression {
            ExpressionNode::Literal(number) => {
                self.hir_set_literal(number);
                Ok(number.clone())
            }
            ExpressionNode::ArrayLiteral(elements) => {
                // The element type expected for this literal, taken from the surrounding array-typed
                // context (`let xs: int[] = ...`, `return ...`, an argument slot, a field, etc.). It
                // is threaded down into each element so nested empty literals (`int[][] = [[]]`) and
                // empty elements infer their element type instead of falling through as untyped.
                let expected_elem = match &self.current_expected_type {
                    Some(Type::Array(elem)) => Some((**elem).clone()),
                    _ => None,
                };

                if elements.is_empty() {
                    // With an array-typed context the empty literal takes that element type; without
                    // one it is genuinely ambiguous (nothing to infer from), so reject it clearly.
                    if let Some(elem) = expected_elem {
                        self.hir_set_empty_array(&elem);
                        return Ok(Type::Array(Box::new(elem)));
                    }
                    self.hir_none();
                    self.hir_fail();
                    diagnostics.report_error(
                        "cannot infer the element type of an empty array literal; add an array type annotation, e.g. `let xs: int[] = [];`".to_string(),
                        expression.position(),
                    );
                    return Ok(Type::Array(Box::new(Type::Void)));
                }

                let saved_expected = self.current_expected_type.take();
                self.current_expected_type = expected_elem;
                let first_type = self.analyze_expression(
                    &elements[0],
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                let mut elem_hirs = vec![self.hir_take()];

                for elem in elements.iter().skip(1) {
                    let element_type =
                        self.analyze_expression(elem, parent_function, symbol_table, diagnostics)?;
                    elem_hirs.push(self.hir_take());
                    self.compare_data_type(&first_type, &element_type, &empty_span(), diagnostics)?;
                }
                self.current_expected_type = saved_expected;

                let array_type = Type::Array(Box::new(first_type));
                self.hir_set_array_lit(elem_hirs, &array_type);
                Ok(array_type)
            }
            ExpressionNode::IndexAccess(array_expr, index_expr) => {
                let array_type = self.analyze_expression(
                    array_expr,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                let array_hir = self.hir_take();

                // A `js`-typed receiver indexes dynamically (`obj[key]`), with a string or numeric
                // key. Must precede the class/string indexer desugar, which would look for a `get`.
                if self.is_js_type(&array_type) {
                    let key_type = self.analyze_expression(
                        index_expr,
                        parent_function,
                        symbol_table,
                        diagnostics,
                    )?;
                    let key_hir = self.hir_take();
                    let _ = key_type;
                    self.desugar_js_index_get(
                        array_hir,
                        key_hir,
                        index_expr.position(),
                        diagnostics,
                    );
                    return Ok(Self::js_type());
                }

                // Class/string indexer: `obj[i]` on a struct or `string` receiver desugars to
                // `obj.get(i)` when an eligible `get` exists (`string` exposes one via `extend
                // string`, yielding a `char`). Arrays keep the built-in index path; `Unknown` is a
                // poison carried from an earlier error and must not cascade.
                if !matches!(array_type, Type::Array(_) | Type::Unknown)
                    && (Self::resolve_struct_parts(&array_type).is_some()
                        || matches!(array_type, Type::String(_)))
                {
                    // The synthesized call re-evaluates the receiver, so drop the base HIR taken above.
                    let _ = array_hir;
                    return self.analyze_index_get(
                        array_expr,
                        index_expr,
                        &array_type,
                        parent_function,
                        symbol_table,
                        diagnostics,
                    );
                }

                let inner_type = match array_type {
                    Type::Array(inner) => *inner,
                    // Don't cascade if the base was already poisoned by an earlier error.
                    Type::Unknown => Type::Unknown,
                    _ => {
                        diagnostics.report_error(
                            format!("Cannot index into non-array type {}", array_type.get_type()),
                            array_expr.position(),
                        );
                        Type::Unknown
                    }
                };

                let index_type = self.analyze_expression(
                    index_expr,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                let index_hir = self.hir_take();
                if !index_type.is_unknown() && !index_type.is_int() {
                    diagnostics.report_error(
                        format!(
                            "Array index must be of type int, got {}",
                            index_type.get_type()
                        ),
                        index_expr.position(),
                    );
                }

                self.hir_set_index(array_hir, index_hir, &inner_type);
                Ok(inner_type)
            }
            ExpressionNode::Unary(opr, right) => {
                let right_type =
                    self.analyze_expression(right, parent_function, symbol_table, diagnostics)?;
                let operand = self.hir_take();
                match opr.kind {
                    TokenKind::BangToken => {
                        if !right_type.is_unknown() && !right_type.is_bool() {
                            diagnostics.report_error(
                                format!("! operator requires bool, got {}", right_type.get_type()),
                                Some(opr.position),
                            );
                        }
                        let result = Type::Boolean(opr.clone());
                        self.hir_set_unary(opr, operand, &result);
                        Ok(result)
                    }
                    TokenKind::PlusToken | TokenKind::MinusToken => {
                        if !right_type.is_unknown()
                            && !matches!(
                                right_type,
                                Type::Integer(_)
                                    | Type::Long(_)
                                    | Type::UInt(_)
                                    | Type::ULong(_)
                                    | Type::Byte(_)
                                    | Type::Float(_)
                                    | Type::Double(_)
                            )
                        {
                            diagnostics.report_error(
                                format!(
                                    "unary +/- requires a numeric type, got {}",
                                    right_type.get_type()
                                ),
                                Some(opr.position),
                            );
                        }
                        self.hir_set_unary(opr, operand, &right_type);
                        Ok(right_type)
                    }
                    _ => {
                        diagnostics.report_error(
                            format!("unknown unary operator {}", opr.text),
                            Some(opr.position),
                        );
                        self.hir_none();
                        Ok(right_type)
                    }
                }
            }
            ExpressionNode::Binary(left, opr, right) => Ok(self.analyze_binary_expression(
                left,
                opr,
                right,
                parent_function,
                symbol_table,
                diagnostics,
            )?),
            ExpressionNode::Identifier(id) => {
                Ok(self.analyze_identifier(id, symbol_table, diagnostics)?)
            }
            ExpressionNode::FunctionCall(name, generic_args, params) => {
                // `analyze_function_call` records the call's HIR itself (only for a resolvable,
                // non-generic, non-overloaded, non-async free function; otherwise it clears `last`).
                let t = self.analyze_function_call(
                    name,
                    generic_args,
                    params,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::IsExpression(left, right_type, _binding) => {
                // `is` always evaluates to a bool. A concrete static operand folds to a compile-time
                // result; an `object` or interface-typed operand emits a runtime `$object_tag`
                // comparison. (The optional `_binding` is handled by the statement layer — `if`/
                // `while` conditions and top-level `&&` chains, see `statements.rs` — which flow-types
                // it into the guarded branch/body; the expression itself ignores the binding here.)
                let left_type =
                    self.analyze_expression(left, parent_function, symbol_table, diagnostics)?;
                let left_hir = self.hir_take();
                let left_name = left_type.get_type();
                let right_name = right_type.get_type();
                let stripped = strip_nullable(&left_name);
                if left_type.is_unknown() {
                    self.hir_none();
                } else if stripped == "object" || self.is_interface_name(stripped) {
                    self.hir_set_is_type(left_hir, right_type);
                } else {
                    self.hir_set_bool(stripped == strip_nullable(&right_name));
                }
                Ok(Type::Boolean(synthetic_token(
                    TokenKind::BooleanToken,
                    "true",
                )))
            }
            ExpressionNode::Parenthesized(expr) => {
                Ok(self.analyze_expression(expr, parent_function, symbol_table, diagnostics)?)
            }
            ExpressionNode::Ternary(condition, then_expr, else_expr) => {
                let cond_type =
                    self.analyze_expression(condition, parent_function, symbol_table, diagnostics)?;
                let cond_hir = self.hir_take();
                if !cond_type.is_bool() {
                    diagnostics.report_error(
                        format!(
                            "Ternary condition must be of type bool, got {}",
                            cond_type.get_type()
                        ),
                        condition.position(),
                    );
                }
                let then_type =
                    self.analyze_expression(then_expr, parent_function, symbol_table, diagnostics)?;
                let then_hir = self.hir_take();
                let else_type =
                    self.analyze_expression(else_expr, parent_function, symbol_table, diagnostics)?;
                let else_hir = self.hir_take();
                // Both branches must agree; reuse the standard compatibility check.
                self.compare_data_type(&then_type, &else_type, &empty_span(), diagnostics)?;
                self.hir_set_ternary(cond_hir, then_hir, else_hir, &then_type);
                Ok(then_type)
            }
            ExpressionNode::Switch(subject, arms) => {
                // `analyze_pattern_switch` desugars the value-position switch and records its result temp read.
                let t = self.analyze_pattern_switch(
                    subject,
                    arms,
                    parent_function,
                    symbol_table,
                    true,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::MemberAccess(obj, member) => {
                // `analyze_member_access` records the HIR itself (struct-field read / enum value).
                let t = self.analyze_member_access(
                    obj,
                    member,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::Cast(target_type, expr) => {
                // `analyze_cast` records the cast's HIR itself.
                let t = self.analyze_cast(
                    target_type,
                    expr,
                    parent_function,
                    symbol_table,
                    diagnostics,
                )?;
                Ok(t)
            }
            ExpressionNode::MethodCall(obj, method, generic_args, params) => {
                let ctx = super::AnalyzerContext {
                    parent_function,
                    symbol_table,
                };
                let t =
                    self.analyze_method_call(obj, method, generic_args, params, &ctx, diagnostics)?;
                // `analyze_method_call` records the `MethodCall`/`Call` (or clears `last`) itself.
                Ok(t)
            }
            ExpressionNode::Await(inner) => {
                let fut =
                    self.analyze_expression(inner, parent_function, symbol_table, diagnostics)?;
                let inner_hir = self.hir_take();
                if fut.is_unknown() {
                    self.hir_none();
                    return Ok(Type::Unknown);
                }
                // Awaiting a dynamic `js` value treats it as a JS Promise: desugar to
                // `await js.__await(inner)`, whose async bridge yields `Future<js>` and resolves to
                // the awaited value as another `js`.
                if self.is_js_type(&fut) {
                    let fut_hir = self.desugar_js_await(inner_hir);
                    let opt = Self::option_js_type();
                    self.hir_set_await(fut_hir, &opt);
                    return Ok(opt);
                }
                match Self::future_inner_type(&fut) {
                    Some(t) => {
                        self.hir_set_await(inner_hir, &t);
                        Ok(t)
                    }
                    None => {
                        self.hir_none();
                        Err(report(
                            diagnostics,
                            format!("'await' expects a Future value, got {}", fut.get_type()),
                            inner.position(),
                        ))
                    }
                }
            }
        }
    }

    /// Desugars a class indexer read `obj[index]` to `obj.get(index)` when `obj_type` exposes an
    /// eligible `get` (see [`Analyzer::resolve_hook_method`]): an accessible instance, non-async
    /// method taking one argument and returning a (non-`void`) value. Any other same-named `get`
    /// (static/async/void/wrong arity) is left as an ordinary method and this site reports why the
    /// value cannot be indexed, rather than silently rewriting the call.
    fn analyze_index_get(
        &mut self,
        array_expr: &'a ExpressionNode<'a>,
        index_expr: &'a ExpressionNode<'a>,
        obj_type: &Type,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let info = match self.resolve_hook_or_diagnose(
            obj_type,
            "get",
            1,
            array_expr.position(),
            true,
            diagnostics,
            |reason| format!("type '{}' cannot be indexed: {}", obj_type.get_type(), reason),
            || {
                format!(
                    "type '{}' has no indexer (define 'public fun get(index): T' to allow obj[index])",
                    obj_type.get_type()
                )
            },
        ) {
            Some(info) => info,
            None => return Ok(Type::Unknown),
        };
        if matches!(info.return_type, None | Some(Type::Void)) {
            self.hir_fail();
            self.hir_none();
            diagnostics.report_error(
                format!(
                    "type '{}' has no indexer: its 'get' must return a value",
                    obj_type.get_type()
                ),
                array_expr.position(),
            );
            return Ok(Type::Unknown);
        }
        let get_tok = synthetic_token(TokenKind::IdentifierToken, "get");
        let call =
            ExpressionNode::MethodCall(array_expr, get_tok, None, vec![(*index_expr).clone()]);
        self.analyze_expression(&call, parent_function, symbol_table, diagnostics)
    }

    /// Types a member access `obj.member`: discriminated-union unit-variant construction
    /// (`Option.None`), enum member access (`Color.Red`), and struct field access (with generic
    /// instantiation and field-privacy enforcement). Returns the accessed field/member type.
    fn analyze_member_access(
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

        let (base_name, generic_args) = match Self::resolve_struct_parts(&obj_type) {
            Some(parts) => parts,
            None => {
                self.hir_none();
                return Err(report(
                    diagnostics,
                    format!(
                        "Cannot access member of non-class type {}",
                        obj_type.get_type()
                    ),
                    Some(member.position),
                ));
            }
        };

        self.ensure_struct_instantiated(&base_name, &generic_args, &member.position, diagnostics);
        let struct_name = mangle_generic(&base_name, &generic_args);

        let field = match self.struct_table.get_struct(&struct_name) {
            Some(info) => info
                .fields
                .get(&member.text)
                .map(|f| (f.type_.clone(), f.is_public)),
            None => {
                self.hir_none();
                return Err(report(
                    diagnostics,
                    format!("Struct '{}' not found", struct_name),
                    Some(member.position),
                ));
            }
        };

        let (field_type, field_is_public) = match field {
            Some(f) => f,
            None => {
                // Not a field: `obj.prop` may read a property getter, which desugars to a call of
                // the (internally named) getter method. The call carries its own privacy/type check.
                let getter = method_fn(&struct_name, &getter_member_name(&member.text));
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
                self.hir_none();
                return Err(report(
                    diagnostics,
                    format!(
                        "Field '{}' not found in class '{}'",
                        member.text, struct_name
                    ),
                    Some(member.position),
                ));
            }
        };

        // Private fields (the default) may only be read from within the declaring type's
        // own methods; `public` exposes them to outside code.
        if !field_is_public && !self.in_methods_of(parent_function, &base_name) {
            diagnostics.report_error(
                format!("'{}' is private to '{}'", member.text, base_name),
                Some(member.position),
            );
        }

        match self.struct_field_index(&struct_name, &member.text) {
            Some(index) => self.hir_set_field(obj_hir, index, &field_type),
            None => self.hir_none(),
        }
        Ok(field_type)
    }

    /// Types a cast `expr as T`: instantiates a generic target struct if needed, then validates the
    /// conversion (identity, numeric<->numeric, `char`<->`int`/`byte`, boxing/unboxing via `object`,
    /// and `int`->pointer for null literals). Always yields the target type, reporting an error for
    /// disallowed conversions so analysis can continue.
    pub(super) fn analyze_cast(
        &mut self,
        target_type: &Type,
        expr: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let expr_type =
            self.analyze_expression(expr, parent_function, symbol_table, diagnostics)?;
        let inner_hir = self.hir_take();

        let target_type_str = target_type.get_type();
        let expr_type_str = expr_type.get_type();

        // If the target (after peeling array wrappers) is a generic struct, instantiate it.
        let mut core_target = target_type;
        while let Type::Array(inner) = core_target {
            core_target = inner;
        }
        if let Some((base_name, generic_args)) = Self::resolve_struct_parts(core_target) {
            self.ensure_struct_instantiated(&base_name, &generic_args, &empty_span(), diagnostics);
        }

        // The cast yields `target_type` regardless of whether the conversion is allowed (a
        // disallowed one is reported below); record its HIR before the validation branches.
        self.hir_set_cast(inner_hir, target_type);

        if target_type_str == expr_type_str ||
           (is_numeric_primitive(&target_type_str) && is_numeric_primitive(&expr_type_str)) ||
           // `char` is a code point: allow lossless conversion to/from `int`/`byte`.
           (target_type_str == "char" && (expr_type_str == "int" || expr_type_str == "byte")) ||
           ((target_type_str == "int" || target_type_str == "byte") && expr_type_str == "char")
        {
            Ok(target_type.clone())
        } else if target_type_str == "object" || expr_type_str == "object" {
            // Boxing (`T as object`) and unboxing (`object as T`) are always permitted;
            // an unbox to the wrong primitive traps at runtime.
            Ok(target_type.clone())
        } else if expr_type_str == "int"
            && (self.struct_table.get_struct(&target_type_str).is_some()
                || target_type_str.ends_with("[]")
                || target_type_str.ends_with("?"))
        {
            // Allow casting int to pointer types (for null pointers)
            Ok(target_type.clone())
        } else if self.is_interface_name(strip_nullable(&target_type_str)) {
            // Cast to an interface (`(Animal)cat`). Allowed from another interface, or a class that
            // implements the interface (an upcast). Both are identity at runtime (same tagged
            // pointer); only the static type changes.
            let src = strip_nullable(&expr_type_str);
            if self.is_interface_name(src)
                || self.implements_as_interface_ref(src, strip_nullable(&target_type_str))
            {
                Ok(target_type.clone())
            } else {
                diagnostics.report_error(
                    format!(
                        "Cannot cast from {} to interface {} ({} does not implement it)",
                        expr_type_str, target_type_str, expr_type_str
                    ),
                    target_type.get_span().or_else(|| expr.position()),
                );
                Ok(target_type.clone())
            }
        } else if self.is_interface_name(strip_nullable(&expr_type_str)) {
            // Downcast from an interface to a concrete class or another interface: permitted
            // (identity at runtime; like unboxing `object`, a wrong downcast is the caller's risk).
            Ok(target_type.clone())
        } else {
            diagnostics.report_error(
                format!("Cannot cast from {} to {}", expr_type_str, target_type_str),
                target_type.get_span().or_else(|| expr.position()),
            );
            Ok(target_type.clone())
        }
    }

    pub(super) fn analyze_binary_expression(
        &mut self,
        left: &ExpressionNode<'a>,
        opr: &SyntaxToken,
        right: &ExpressionNode<'a>,
        parent_function: &FunctionNode<'a>,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let left_value =
            self.analyze_expression(left, parent_function, symbol_table, diagnostics)?;
        let left_hir = self.hir_take();
        let right_value =
            self.analyze_expression(right, parent_function, symbol_table, diagnostics)?;
        let right_hir = self.hir_take();

        // Null-coalescing `a ?? b`: `a` should be nullable; the result is the unwrapped element
        // type, and `b` must be assignable to it (or itself nullable of the same element type).
        if opr.kind == TokenKind::QuestionQuestionToken {
            let result_type = match &left_value {
                Type::Nullable(inner) => (**inner).clone(),
                other => other.clone(),
            };
            let right_unwrapped = match &right_value {
                Type::Nullable(inner) => (**inner).clone(),
                other => other.clone(),
            };
            self.compare_data_type(&result_type, &right_unwrapped, &opr.position, diagnostics)?;
            self.hir_set_coalesce(left_hir, right_hir, &result_type);
            return Ok(result_type);
        }

        // String concatenation: `string + T` (or `T + string`) yields a string, auto-converting
        // the non-string operand through its `to_string` (the object protocol, or a C-style enum's
        // variant-name rendering) in codegen. This means `"count = " + n` works for any `n` with no
        // explicit `.to_string()`.
        if opr.kind == TokenKind::PlusToken {
            let left_is_string = left_value.is_string();
            let right_is_string = right_value.is_string();
            if left_is_string || right_is_string {
                self.hir_set_concat(left_hir, &left_value, right_hir, &right_value);
                return Ok(if left_is_string {
                    left_value
                } else {
                    right_value
                });
            }
        }

        self.compare_data_type(&left_value, &right_value, &opr.position, diagnostics)?;
        match (&left_value, &opr.kind) {
            (Type::String(_), TokenKind::PlusToken) => {}
            // Reference (identity) equality is allowed on strings and objects.
            (Type::String(_), TokenKind::EqualEqualToken)
            | (Type::String(_), TokenKind::NotEqualToken) => {}
            (Type::String(_), _) => {
                diagnostics.report_error(
                    format!("Cannot perform operation {} on string", opr.text),
                    Some(opr.position),
                );
            }
            (_, _) => {}
        };

        // User-defined value equality: for `==`/`!=` where the operand's static type is a user type
        // that implements `Equatable<Self>`, dispatch to its `equals` method (a static call),
        // negating the result for `!=`. Primitives, strings, and null comparisons keep the built-in
        // behavior handled above/below.
        if matches!(
            opr.kind,
            TokenKind::EqualEqualToken | TokenKind::NotEqualToken
        ) {
            if let Some(equals_fn) = self.equatable_equals_fn(&left_value) {
                let bool_ty = Type::Boolean(opr.clone());
                self.hir_set_method_call(left_hir, &equals_fn, vec![right_hir], &bool_ty);
                if opr.kind == TokenKind::NotEqualToken {
                    self.hir_negate_last();
                }
                return Ok(bool_ty);
            }
        }

        let is_bool_result = matches!(
            opr.kind,
            TokenKind::EqualEqualToken
                | TokenKind::NotEqualToken
                | TokenKind::GreaterThanToken
                | TokenKind::GreaterThanEqualToken
                | TokenKind::SmallerThanToken
                | TokenKind::SmallerThanEqualToken
                | TokenKind::AmpersandAmpersandToken
                | TokenKind::PipePipeToken
        );
        let result_type = if is_bool_result {
            Type::Boolean(opr.clone())
        } else {
            left_value.clone()
        };
        self.hir_set_binary(left_hir, opr, right_hir, &result_type);
        Ok(result_type)
    }

    /// If `==`/`!=` on a value of type `left` should dispatch to a user-defined `equals`, returns
    /// the mangled method symbol (e.g. `Money_equals`). Applies when `left`'s concrete (non-nullable)
    /// type is a class/struct that implements `Equatable<Self>`; the caller has already verified the
    /// operands are type-compatible.
    fn equatable_equals_fn(&self, left: &Type) -> Option<String> {
        let (base, args) = Self::resolve_struct_parts(left)?;
        let recv = mangle_generic(&base, &args);
        // The interface argument is the receiver type itself (the `Equatable<Self>` convention),
        // mangled exactly as `validate_implements` recorded it.
        let self_ty = match left {
            Type::Nullable(inner) => (**inner).clone(),
            other => other.clone(),
        };
        let iface = mangle_generic("Equatable", std::slice::from_ref(&self_ty));
        if self.class_implements(&recv, &iface) {
            return Some(method_fn(&recv, "equals"));
        }
        None
    }

    pub(super) fn compare_data_type(
        &mut self,
        left: &Type,
        right: &Type,
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        // A poison operand (from an earlier reported error) is compatible with anything, so we
        // never emit a follow-on mismatch for it.
        if left.is_unknown() || right.is_unknown() {
            return Ok(());
        }

        // Directional assignability over interned types: `right` (value) must be assignable to
        // `left` (target). Covers identity, `object` widening, enum/int, numeric widening, and
        // nullable/`null` handling via the structured rules.
        let l = self.type_ctx.lower(left);
        let r = self.type_ctx.lower(right);
        if crate::types::assignable(&self.type_ctx.interner, l, r) {
            return Ok(());
        }
        // `compare_data_type` also backs equality comparisons (`ref == null`, `null == ref`),
        // where `null` may appear on either side. Accept the reverse direction, but only for the
        // `null`-literal case so a narrowing assignment is still rejected.
        if (left.get_type() == "void?" || right.get_type() == "void?")
            && crate::types::assignable(&self.type_ctx.interner, r, l)
        {
            return Ok(());
        }

        // Implicit upcast to an interface: a value whose concrete class implements the interface
        // `left` is assignable to it (`let a: Animal = cat;`).
        if self.value_assignable_to_interface(left, right) {
            return Ok(());
        }

        diagnostics.report_error(
            format!(
                "cannot convert from {} to {} at {}",
                left.display_name(),
                right.display_name(),
                position.get_point_str()
            ),
            Some(*position),
        );
        Ok(())
    }

    /// Resolves a field's position in a struct's layout (offset order, matching the
    /// auto-generated constructor's argument order and the backend's field indexing). Returns
    /// `None` if the struct or field is unknown.
    pub(super) fn struct_field_index(&self, struct_name: &str, field: &str) -> Option<usize> {
        let info = self.struct_table.get_struct(struct_name)?;
        let mut ordered: Vec<(&String, &crate::semantics::struct_table::StructFieldInfo)> =
            info.fields.iter().collect();
        ordered.sort_by_key(|(_, f)| f.offset);
        ordered.iter().position(|(n, _)| n.as_str() == field)
    }
    pub(super) fn analyze_identifier(
        &mut self,
        id: &SyntaxToken,
        symbol_table: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<Type, SemanticError> {
        let r = match (*symbol_table).as_ref().borrow().get_symbol(id) {
            Ok(t) => t,
            Err(e) => {
                // A bare identifier that names a top-level function is a first-class function value.
                if let Ok(sig) = self.function_table.get_function(&id.text) {
                    let params = sig
                        .parameters
                        .iter()
                        .map(|p| Self::type_from_name(p))
                        .collect();
                    let ret = sig.return_type.clone().unwrap_or(Type::Void);
                    let func_ty = Type::Function(params, Box::new(ret.clone()));
                    self.hir_set_func_value(&id.text, &func_ty, &ret);
                    return Ok(func_ty);
                }
                // A generic function used as a value (`let cmp: fun(T, T): int = natural_order;`):
                // infer its type arguments from the expected function type and instantiate it.
                if self.generic_functions.contains_key(&id.text) {
                    if let Some(func_ty) = self.instantiate_generic_function_value(id, diagnostics)
                    {
                        return Ok(func_ty);
                    }
                }
                // Unresolved name: report and short-circuit. Statement-level callers recover
                // (poisoning the binding with `Type::Unknown`) so sibling errors still surface.
                return Err(report(diagnostics, e.to_string(), Some(id.position)));
            }
        };
        // File/module-level visibility (Axis 2): a non-public top-level variable is only readable
        // from its declaring file. (Locals/params never appear in `self.globals`, so a shadowing
        // local of the same name is unaffected.)
        if let Some(global) = self.globals.iter().find(|g| g.name == id.text) {
            if !self.visible_across_files(
                &global.file_path,
                global.is_public,
                self.current_file.as_ref(),
            ) {
                let decl_file = global.file_path.clone();
                self.report_not_public("Variable", &id.text, &decl_file, id.position, diagnostics);
            }
        }
        self.hir_set_var(&id.text);
        Ok(r)
    }

    /// Reconstructs a `Type` from its canonical type-name string (as stored in function-table
    /// signatures), e.g. "int", "string", "Node", "int[]". Falls back to `void` if unparseable.
    pub(super) fn type_from_name(name: &str) -> Type {
        let token = synthetic_token(TokenKind::IdentifierToken, name);
        Type::from_token(token).unwrap_or(Type::Void)
    }
}
