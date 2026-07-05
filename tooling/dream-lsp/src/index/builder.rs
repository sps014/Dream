//! Builds the span-indexed symbol model by walking the parsed AST: records declarations and
//! references, infers variable types, and emits inlay hints. Best-effort and tolerant of
//! partially-broken trees.

use std::collections::HashMap;

use dream::syntax::nodes::struct_node::StructDeclarationNode;
use dream::syntax::nodes::types::CONSTRUCTOR_NAME;
use dream::syntax::nodes::{
    ExpressionNode, FunctionNode, PatternNode, ProgramNode, StatementNode, SwitchArmBody, Type,
};
use dream::syntax::token::syntax_token::SyntaxToken;

use super::{
    base_struct, fn_value_type, param_names, signature, Decl, Index, InlayHintOut, InlayKind, Ref,
    SymKind, GLOBAL,
};

pub(crate) struct Builder {
    pub(crate) decls: Vec<Decl>,
    pub(crate) refs: Vec<Ref>,
    pub(crate) inlay_hints: Vec<InlayHintOut>,
    pub(crate) next_scope: usize,
    pub(crate) is_main: bool,
    /// Parameter names per free function name, used to render parameter-name inlay hints at calls.
    pub(crate) fn_params: HashMap<String, Vec<String>>,
    /// Parameter names per method name (the implicit `this` is not a parsed parameter).
    pub(crate) method_params: HashMap<String, Vec<String>>,
    /// Constructor parameter names per struct name (only when a custom `constructor` is declared).
    pub(crate) ctor_params: HashMap<String, Vec<String>>,
}

impl Builder {
    fn infer_type(&self, expr: &ExpressionNode, scope: usize) -> Option<String> {
        self.infer_type_internal(expr, scope)
    }

    fn infer_type_internal(&self, expr: &ExpressionNode, scope: usize) -> Option<String> {
        match expr {
            ExpressionNode::Literal(t) => Some(t.display_name()),
            ExpressionNode::Cast(ty, _) => Some(ty.display_name()),
            ExpressionNode::IsExpression(_, _, _) => Some("bool".to_string()),
            ExpressionNode::Binary(left, op, right) => match op.kind {
                dream::syntax::token::token_kind::TokenKind::EqualEqualToken
                | dream::syntax::token::token_kind::TokenKind::NotEqualToken
                | dream::syntax::token::token_kind::TokenKind::GreaterThanToken
                | dream::syntax::token::token_kind::TokenKind::GreaterThanEqualToken
                | dream::syntax::token::token_kind::TokenKind::SmallerThanToken
                | dream::syntax::token::token_kind::TokenKind::SmallerThanEqualToken
                | dream::syntax::token::token_kind::TokenKind::AmpersandAmpersandToken
                | dream::syntax::token::token_kind::TokenKind::PipePipeToken => {
                    Some("bool".to_string())
                }
                // Arithmetic operators (`+ - * /`) yield the type of their left operand, mirroring
                // the compiler's `analyze_binary_expression` (result type = left operand type). This
                // is what makes `let a = c * 5` infer `int` for hover/inlay hints. Fall back to the
                // right operand when the left is unresolvable.
                dream::syntax::token::token_kind::TokenKind::PlusToken
                | dream::syntax::token::token_kind::TokenKind::MinusToken
                | dream::syntax::token::token_kind::TokenKind::StarToken
                | dream::syntax::token::token_kind::TokenKind::SlashToken => self
                    .infer_type(left, scope)
                    .or_else(|| self.infer_type(right, scope)),
                _ => None,
            },
            ExpressionNode::Identifier(token) => self
                .resolve(&token.text, scope, token.position.start)
                .and_then(|d| d.ty.clone()),
            ExpressionNode::MemberAccess(_recv, member) => {
                // To properly type `obj.field`, we'd resolve `obj`'s type, then find the field in that struct.
                // For a simple heuristic, just find *any* field with this name.
                self.decls
                    .iter()
                    .find(|d| d.name == member.text && d.kind == SymKind::Field)
                    .and_then(|d| d.ty.clone())
            }
            ExpressionNode::FunctionCall(name, generic_args, _) => {
                self.resolve(&name.text, scope, name.position.start)
                    .and_then(|d| {
                        if d.kind == SymKind::Struct {
                            // It's a constructor call (e.g. `Test("John", 20)`), so the type is the struct
                            // name itself, rendered with angle brackets when generic (`Box<int>`).
                            match generic_args {
                                Some(args) => {
                                    let args_str = args
                                        .iter()
                                        .map(|a| a.display_name())
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    Some(format!("{}<{}>", name.text, args_str))
                                }
                                None => Some(name.text.clone()),
                            }
                        } else {
                            // detail string usually looks like: fun(int, int): string
                            if let Some(colon_idx) = d.detail.rfind(':') {
                                let mut ret_ty = d.detail[colon_idx + 1..].trim().to_string();
                                if let Some(args) = generic_args {
                                    if args.len() == 1 {
                                        let arg_type = args[0].display_name();
                                        ret_ty = ret_ty
                                            .replace("<T>", &format!("<{}>", arg_type))
                                            .replace(" T", &format!(" {}", arg_type))
                                            .replace("T ", &format!("{} ", arg_type));
                                        if ret_ty == "T" {
                                            ret_ty = arg_type.to_string();
                                        }
                                    }
                                }
                                Some(ret_ty)
                            } else {
                                None
                            }
                        }
                    })
            }
            ExpressionNode::MethodCall(recv, method, _, _) => {
                let receiver_ty_opt = self.infer_type(recv, scope);
                self.decls
                    .iter()
                    .find(|d| d.name == method.text && d.kind == SymKind::Method)
                    .and_then(|d| {
                        let detail = if let Some(receiver_ty) = &receiver_ty_opt {
                            Index::substitute_generic(&d.detail, receiver_ty)
                        } else {
                            d.detail.clone()
                        };
                        detail
                            .rfind(':')
                            .map(|colon_idx| detail[colon_idx + 1..].trim().to_string())
                    })
            }
            ExpressionNode::Parenthesized(inner) => self.infer_type(inner, scope),
            ExpressionNode::Await(inner) => {
                // `await` unwraps a `Future<T>` to `T`. Call inference already reports an async
                // function's *declared* return type (e.g. `int` for `async fun f(): int`), so the
                // inner type is usually the awaited type already; only an explicit `Future<T>`
                // needs unwrapping.
                let inner_ty = self.infer_type(inner, scope)?;
                let unwrapped = inner_ty
                    .strip_prefix("Future<")
                    .and_then(|rest| rest.strip_suffix('>'))
                    .map(|t| t.to_string())
                    .unwrap_or(inner_ty);
                Some(unwrapped)
            }
            _ => None,
        }
    }

    fn resolve(&self, name: &str, scope: usize, before: usize) -> Option<&Decl> {
        let local = self
            .decls
            .iter()
            .filter(|d| {
                d.name == name
                    && d.scope == scope
                    && matches!(d.kind, SymKind::Variable | SymKind::Param)
                    && d.start <= before
            })
            .max_by_key(|d| d.start);
        if local.is_some() {
            return local;
        }
        // File-scope fallback: free functions, types, and top-level `let`/`const` globals (which
        // carry `scope == GLOBAL` and `SymKind::Variable`).
        self.decls.iter().find(|d| {
            d.name == name
                && d.scope == GLOBAL
                && matches!(
                    d.kind,
                    SymKind::Function | SymKind::Struct | SymKind::Enum | SymKind::Variable
                )
        })
    }

    pub(crate) fn walk_program_for_imports(&mut self, program: &ProgramNode) {
        for func in &program.functions {
            let detail = signature(func);
            let fn_ty = fn_value_type(func);
            self.push_decl(&func.name, SymKind::Function, detail, GLOBAL, Some(fn_ty));
            self.fn_params
                .insert(func.name.text.clone(), param_names(func));
        }
        for st in &program.structs {
            let detail = format!("class {}", st.name.text);
            self.push_decl(&st.name, SymKind::Struct, detail, GLOBAL, None);
            for field in &st.fields {
                let field_ty = field.field_type.display_name();
                let detail = format!("{}.{}: {}", st.name.text, field.name.text, field_ty);
                self.push_decl(&field.name, SymKind::Field, detail, GLOBAL, Some(field_ty));
            }
            for method in &st.methods {
                let detail = format!("{}.{}", st.name.text, signature(method));
                self.push_decl(&method.name, SymKind::Method, detail, GLOBAL, None);
                if method.name.text == CONSTRUCTOR_NAME {
                    self.ctor_params
                        .insert(st.name.text.clone(), param_names(method));
                } else {
                    self.method_params
                        .insert(method.name.text.clone(), param_names(method));
                }
            }
        }
        for en in &program.enums {
            let generics = en
                .generic_parameters
                .as_ref()
                .map(|params| {
                    let names = params
                        .iter()
                        .map(|p| p.text.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("<{}>", names)
                })
                .unwrap_or_default();
            let detail = format!("enum {}{}", en.name.text, generics);
            self.push_decl(&en.name, SymKind::Enum, detail, GLOBAL, None);
            for variant in &en.variants {
                let detail = if variant.fields.is_empty() {
                    format!("{}.{} = {}", en.name.text, variant.name.text, variant.value)
                } else {
                    let params = variant
                        .fields
                        .iter()
                        .map(|f| format!("{}: {}", f.name.text, f.field_type.display_name()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}.{}({})", en.name.text, variant.name.text, params)
                };
                self.push_decl(&variant.name, SymKind::EnumMember, detail, GLOBAL, None);
            }
        }
        for ext in &program.extends {
            for method in &ext.methods {
                let detail = format!("{}.{}", ext.target.text, signature(method));
                self.push_decl(&method.name, SymKind::Method, detail, GLOBAL, None);
                self.method_params
                    .insert(method.name.text.clone(), param_names(method));
            }
        }
        // Top-level `let`/`const` variables live at file scope and are visible from every
        // function body, so they are declared here in pass 1 alongside the other globals.
        for global in &program.globals {
            let ty = global
                .declared_type
                .as_ref()
                .map(|t| t.display_name())
                .or_else(|| self.infer_type(&global.initializer, GLOBAL));
            let keyword = if global.is_const { "const" } else { "let" };
            let detail = match &ty {
                Some(t) => format!("{} {}: {}", keyword, global.name.text, t),
                None => format!("{} {}", keyword, global.name.text),
            };
            self.push_decl(&global.name, SymKind::Variable, detail, GLOBAL, ty);
        }
    }

    fn walk_attributes(
        &mut self,
        attributes: &[dream::syntax::nodes::AttributeNode],
        scope: usize,
    ) {
        for attr in attributes {
            // Treat the attribute name as a reference to a struct/class (even if it's currently a built-in).
            self.add_ref(&attr.name, SymKind::Struct, scope);
            for arg in &attr.args {
                // If the argument is an identifier (e.g. referencing a constant), record it.
                if arg.kind == dream::syntax::token::token_kind::TokenKind::IdentifierToken {
                    self.add_ref(arg, SymKind::Variable, scope);
                }
            }
        }
    }

    pub(crate) fn walk_program(&mut self, program: &ProgramNode) {
        for func in &program.functions {
            self.walk_attributes(&func.attributes, GLOBAL);
            self.walk_function(func, None);
        }
        for st in &program.structs {
            self.walk_attributes(&st.attributes, GLOBAL);
            for field in &st.fields {
                self.walk_attributes(&field.attributes, GLOBAL);
            }
            self.walk_struct(st);
        }
        for _en in &program.enums {
            // Already declared in pass 1
        }
        for ext in &program.extends {
            for method in &ext.methods {
                self.walk_attributes(&method.attributes, GLOBAL);
                self.walk_method(method, &ext.target.text);
            }
        }
        // Walk each top-level initializer at file scope so identifiers inside it become references,
        // and emit a type inlay hint when the variable has no explicit annotation.
        for global in &program.globals {
            if global.declared_type.is_none() {
                if let Some(t) = self.infer_type(&global.initializer, GLOBAL) {
                    self.inlay_hints.push(InlayHintOut {
                        offset: global.name.position.end,
                        label: format!(": {}", t),
                        kind: InlayKind::Type,
                    });
                }
            } else if let Some(t) = &global.declared_type {
                self.add_type_ref(t, GLOBAL);
            }
            self.walk_expr(&global.initializer, GLOBAL);
        }
    }

    fn walk_struct(&mut self, st: &StructDeclarationNode) {
        for method in &st.methods {
            self.walk_method(method, &st.name.text);
        }
    }

    fn walk_method(&mut self, func: &FunctionNode, owner: &str) {
        let scope = self.fresh_scope();
        // Instance methods receive an implicit `this` bound to the owning type, so member
        // access on `this` can be resolved to the owner's fields/methods. Static methods do not.
        if !func.is_static {
            self.decls.push(Decl {
                name: "this".to_string(),
                kind: SymKind::Param,
                detail: format!("(this) {}", owner),
                doc_comment: None,
                start: func.name.position.start,
                end: func.name.position.end,
                scope,
                ty: Some(owner.to_string()),
                is_main: self.is_main,
            });
        }
        self.walk_params_and_body(func, scope);
    }

    fn walk_function(&mut self, func: &FunctionNode, _owner: Option<&str>) {
        let scope = self.fresh_scope();
        self.walk_params_and_body(func, scope);
    }

    fn walk_params_and_body(&mut self, func: &FunctionNode, scope: usize) {
        for param in &func.parameters {
            let ty = param.type_.display_name();
            let detail = format!("(parameter) {}: {}", param.name.text, ty);
            self.push_decl(&param.name, SymKind::Param, detail, scope, Some(ty));
            self.add_type_ref(&param.type_, scope);
        }
        if let Some(rt) = &func.return_type {
            self.add_type_ref(rt, scope);
        }
        for stmt in func.body {
            self.walk_stmt(stmt, scope);
        }
    }

    fn walk_stmt(&mut self, stmt: &StatementNode, scope: usize) {
        match stmt {
            StatementNode::Declaration(name, ty, expr, _is_const) => {
                let inferred = self.infer_type(expr, scope);
                let type_str = ty
                    .as_ref()
                    .map(|t| t.display_name())
                    .or_else(|| inferred.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                let detail = type_str.clone();
                let resolved_ty = ty.as_ref().map(|t| t.display_name()).or(inferred);
                self.push_decl(name, SymKind::Variable, detail, scope, resolved_ty.clone());
                if let Some(t) = ty {
                    self.add_type_ref(t, scope);
                } else if let Some(t_str) = resolved_ty {
                    self.inlay_hints.push(InlayHintOut {
                        offset: name.position.end,
                        label: format!(": {}", t_str),
                        kind: InlayKind::Type,
                    });
                }
                self.walk_expr(expr, scope);
            }
            StatementNode::Assignment(name, expr) => {
                self.add_ref(name, SymKind::Variable, scope);
                self.walk_expr(expr, scope);
            }
            StatementNode::IndexAssignment(target, index, value) => {
                self.walk_expr(target, scope);
                self.walk_expr(index, scope);
                self.walk_expr(value, scope);
            }
            StatementNode::MemberAssignment(target, member, value) => {
                self.walk_expr(target, scope);
                self.add_ref(member, SymKind::Field, scope);
                self.walk_expr(value, scope);
            }
            StatementNode::Return(Some(expr)) => self.walk_expr(expr, scope),
            StatementNode::Return(None) => {}
            StatementNode::FunctionInvocation(name, _, args) => {
                self.add_ref(name, SymKind::Function, scope);
                let params = self
                    .fn_params
                    .get(&name.text)
                    .or_else(|| self.ctor_params.get(&name.text));
                if let Some(params) = params {
                    self.push_param_hints(&params.clone(), args);
                }
                for arg in args {
                    self.walk_expr(arg, scope);
                }
            }
            StatementNode::ExpressionStatement(expr) => {
                self.walk_expr(expr, scope);
            }
            StatementNode::MethodInvocation(recv, method, _, args) => {
                self.walk_expr(recv, scope);
                // `Enum.Variant(...)` is a variant constructor, not a method call.
                let kind = match recv {
                    ExpressionNode::Identifier(id) if self.is_enum(&id.text) => SymKind::EnumMember,
                    _ => SymKind::Method,
                };
                self.add_ref(method, kind, scope);
                if let Some(params) = self.method_params.get(&method.text) {
                    self.push_param_hints(&params.clone(), args);
                }
                for arg in args {
                    self.walk_expr(arg, scope);
                }
            }
            StatementNode::IfElse(cond, then_body, else_ifs, else_body) => {
                self.walk_expr(cond, scope);
                self.walk_block(then_body, scope);
                for (c, body) in else_ifs {
                    self.walk_expr(c, scope);
                    self.walk_block(body, scope);
                }
                if let Some(body) = else_body {
                    self.walk_block(body, scope);
                }
            }
            StatementNode::While(cond, body) => {
                self.walk_expr(cond, scope);
                self.walk_block(body, scope);
            }
            StatementNode::DoWhile(body, cond) => {
                self.walk_block(body, scope);
                self.walk_expr(cond, scope);
            }
            StatementNode::For(init, cond, update, body) => {
                if let Some(s) = init {
                    self.walk_stmt(s, scope);
                }
                if let Some(c) = cond {
                    self.walk_expr(c, scope);
                }
                if let Some(s) = update {
                    self.walk_stmt(s, scope);
                }
                self.walk_block(body, scope);
            }
            StatementNode::ForEach(var, iterable, _, _, body) => {
                let detail = "unknown".to_string();
                self.push_decl(var, SymKind::Variable, detail, scope, None);
                self.walk_expr(iterable, scope);
                self.walk_block(body, scope);
            }
            StatementNode::Labeled(_, inner) => self.walk_stmt(inner, scope),
            StatementNode::AwaitStmt(expr) => self.walk_expr(expr, scope),
            StatementNode::Break(_) | StatementNode::Continue(_) => {}
            StatementNode::Switch(subject, cases, default) => {
                self.walk_expr(subject, scope);
                for (labels, body) in cases {
                    for label in labels {
                        self.walk_expr(label, scope);
                    }
                    self.walk_block(body, scope);
                }
                if let Some(body) = default {
                    self.walk_block(body, scope);
                }
            }
        }
    }

    fn walk_block(&mut self, body: &[StatementNode], scope: usize) {
        for stmt in body {
            self.walk_stmt(stmt, scope);
        }
    }

    /// Emits a parameter-name inlay hint (`name:`) before each positional argument of a call. The
    /// hint is suppressed when the argument is simply the identifier matching the parameter name,
    /// which would be redundant. Extra arguments (more than parameters) are left unannotated.
    fn push_param_hints(&mut self, params: &[String], args: &[ExpressionNode]) {
        for (param, arg) in params.iter().zip(args.iter()) {
            if let ExpressionNode::Identifier(tok) = arg {
                if &tok.text == param {
                    continue;
                }
            }
            if let Some(span) = arg.start_position() {
                self.inlay_hints.push(InlayHintOut {
                    offset: span.start,
                    label: format!("{}:", param),
                    kind: InlayKind::Parameter,
                });
            }
        }
    }

    fn walk_expr(&mut self, expr: &ExpressionNode, scope: usize) {
        match expr {
            ExpressionNode::Identifier(token) => self.add_ref(token, SymKind::Variable, scope),
            ExpressionNode::Binary(l, _, r) => {
                self.walk_expr(l, scope);
                self.walk_expr(r, scope);
            }
            ExpressionNode::Unary(_, e) | ExpressionNode::Parenthesized(e) => {
                self.walk_expr(e, scope)
            }
            ExpressionNode::FunctionCall(name, _, args) => {
                self.add_ref(name, SymKind::Function, scope);
                // A name resolves to a free function if one exists; otherwise `Name(...)` is a
                // constructor call, whose positional arguments are the custom `constructor`'s
                // parameters. A class with no explicit `constructor` has an implicit zero-arg
                // default constructor, so it contributes no positional parameter hints.
                let params = self
                    .fn_params
                    .get(&name.text)
                    .or_else(|| self.ctor_params.get(&name.text));
                if let Some(params) = params {
                    self.push_param_hints(&params.clone(), args);
                }
                for arg in args {
                    self.walk_expr(arg, scope);
                }
            }
            ExpressionNode::IndexAccess(arr, idx) => {
                self.walk_expr(arr, scope);
                self.walk_expr(idx, scope);
            }
            ExpressionNode::Cast(ty, e) => {
                self.add_type_ref(ty, scope);
                self.walk_expr(e, scope);
            }
            ExpressionNode::MemberAccess(recv, member) => {
                self.walk_expr(recv, scope);
                // `Enum.Member` looks like member access on an identifier naming the enum.
                let kind = match recv {
                    ExpressionNode::Identifier(id) if self.is_enum(&id.text) => SymKind::EnumMember,
                    _ => SymKind::Field,
                };
                self.add_ref(member, kind, scope);
            }
            ExpressionNode::MethodCall(recv, method, _, args) => {
                self.walk_expr(recv, scope);
                // `Enum.Variant(...)` is a variant constructor, not a method call.
                let kind = match recv {
                    ExpressionNode::Identifier(id) if self.is_enum(&id.text) => SymKind::EnumMember,
                    _ => SymKind::Method,
                };
                self.add_ref(method, kind, scope);
                if let Some(params) = self.method_params.get(&method.text) {
                    self.push_param_hints(&params.clone(), args);
                }
                for arg in args {
                    self.walk_expr(arg, scope);
                }
            }
            ExpressionNode::IsExpression(e, ty, _) => {
                self.walk_expr(e, scope);
                self.add_type_ref(ty, scope);
            }
            ExpressionNode::Ternary(c, t, e) => {
                self.walk_expr(c, scope);
                self.walk_expr(t, scope);
                self.walk_expr(e, scope);
            }
            ExpressionNode::ArrayLiteral(elems) => {
                for elem in elems {
                    self.walk_expr(elem, scope);
                }
            }
            ExpressionNode::Await(e) => self.walk_expr(e, scope),
            ExpressionNode::Switch(subject, arms) => {
                self.walk_expr(subject, scope);
                for arm in arms {
                    self.walk_pattern(&arm.pattern, scope);
                    if let Some(guard) = &arm.guard {
                        self.walk_expr(guard, scope);
                    }
                    match &arm.body {
                        SwitchArmBody::Expr(e) => self.walk_expr(e, scope),
                        SwitchArmBody::Block(stmts) => self.walk_block(stmts, scope),
                    }
                }
            }
            ExpressionNode::Literal(_) => {}
        }
    }

    /// Indexes the bindings and variant references introduced by a match pattern so hover, rename,
    /// and go-to work for them. Binding identifiers become local variables; variant names (and an
    /// optional `Enum.` qualifier) become references.
    fn walk_pattern(&mut self, pattern: &PatternNode, scope: usize) {
        match pattern {
            PatternNode::Wildcard(_) | PatternNode::Literal(_) => {}
            PatternNode::Binding(name) => {
                self.push_decl(name, SymKind::Variable, "binding".to_string(), scope, None);
            }
            PatternNode::Variant(qualifier, variant, subs) => {
                if let Some(q) = qualifier {
                    self.add_ref(q, SymKind::Type, scope);
                }
                self.add_ref(variant, SymKind::EnumMember, scope);
                for sub in subs {
                    self.walk_pattern(sub, scope);
                }
            }
        }
    }

    fn add_type_ref(&mut self, ty: &Type, scope: usize) {
        if let Type::Struct(token, _) = base_struct(ty) {
            self.add_ref(token, SymKind::Type, scope);
        }
    }

    fn is_enum(&self, name: &str) -> bool {
        self.decls
            .iter()
            .any(|d| d.kind == SymKind::Enum && d.name == name)
    }

    fn fresh_scope(&mut self) -> usize {
        let scope = self.next_scope;
        self.next_scope += 1;
        scope
    }

    fn push_decl(
        &mut self,
        token: &SyntaxToken,
        kind: SymKind,
        detail: String,
        scope: usize,
        ty: Option<String>,
    ) {
        if token.text.is_empty() {
            return;
        }

        let doc_comment = Self::doc_comment_from_trivia(token);
        self.decls.push(Decl {
            name: token.text.clone(),
            kind,
            detail,
            doc_comment,
            start: token.position.start,
            end: token.position.end,
            scope,
            ty,
            is_main: self.is_main,
        });
    }

    /// Extracts the doc comment attached to `token`, i.e. the trailing run of leading comment
    /// trivia that is *contiguous* — each comment immediately followed by the next, with no blank
    /// line in between. All the comments in `leading_trivia` sit directly before the declaration
    /// with no other real token between them (otherwise the lexer would have attached them
    /// elsewhere), so a blank line is the only thing that can separate two trivia comments; when it
    /// does, everything before that gap belongs to an earlier, disconnected block (e.g. a
    /// file-level header) and must not be glued onto the declaration's doc comment.
    fn doc_comment_from_trivia(token: &SyntaxToken) -> Option<String> {
        use dream::syntax::token::token_kind::TokenKind;

        let comments: Vec<_> = token
            .leading_trivia
            .iter()
            .filter(|t| {
                t.kind == TokenKind::LineCommentToken || t.kind == TokenKind::BlockCommentToken
            })
            .collect();
        if comments.is_empty() {
            return None;
        }

        // Walk backwards from the comment closest to the declaration, stopping at the first blank
        // line — i.e. where a comment's end line isn't immediately followed by the next one.
        let mut start = comments.len() - 1;
        while start > 0 {
            let prev = comments[start - 1];
            let cur = comments[start];
            let prev_end_line = prev.position.line_no + prev.text.matches('\n').count();
            if prev_end_line + 1 != cur.position.line_no {
                break;
            }
            start -= 1;
        }

        let mut doc_comment = String::new();
        for c in &comments[start..] {
            let mut text = c.text.trim();
            if text.starts_with("//") {
                text = text.trim_start_matches('/').trim_start();
            } else if text.starts_with("/*") {
                text = text.trim_start_matches("/*").trim_end_matches("*/").trim();
            }
            if !doc_comment.is_empty() {
                doc_comment.push_str("\n\n");
            }
            doc_comment.push_str(text);
        }
        if doc_comment.is_empty() {
            None
        } else {
            Some(doc_comment)
        }
    }

    fn add_ref(&mut self, token: &SyntaxToken, kind: SymKind, scope: usize) {
        if token.text.is_empty() {
            return;
        }
        self.refs.push(Ref {
            name: token.text.clone(),
            kind,
            start: token.position.start,
            end: token.position.end,
            scope,
            is_main: self.is_main,
        });
    }
}
