use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::function_table::FunctionTable;
use crate::semantics::struct_table::StructTable;
use crate::semantics::symbol_table::SymbolTable;
use crate::semantics::union_table::UnionTable;
use crate::syntax::nodes::types::{mangle_with_suffixes, primitive_type, FUTURE_TYPE};
use crate::syntax::nodes::{EnumDeclarationNode, ExtendNode};
use crate::syntax::nodes::{FunctionNode, ProgramNode, Type};
use crate::syntax::syntax_tree::SyntaxTree;
use crate::syntax::token::syntax_token::SyntaxToken;
use crate::syntax::token::token_kind::TokenKind;
use crate::text::line_text::LineText;
use crate::text::text_span::TextSpan;
use crate::types::{DefKind, TypeCtx};
use bumpalo::Bump;
use indexmap::IndexMap;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

mod await_rules;
mod calls;
mod declarations;
mod expressions;
mod generics;
mod hir_emit;
mod js_interop;
mod register_interfaces;
mod register_methods;
mod statements;
mod switch_exhaustiveness;
mod switch_unions;
mod type_checker;

/// Converts an AST node's `Rc<str>` source-file tag into the `String` form stored on the
/// diagnostic bag (used to attribute each semantic error to its originating file).
fn file_path_string(file_path: &Option<Rc<str>>) -> Option<String> {
    file_path.as_ref().map(|p| p.to_string())
}

/// Reports `message` at `span` into the bag and returns the matching typed [`SemanticError`], so a
/// failing analysis site can `return Err(report(diagnostics, msg, span))` in a single step. The
/// pushed diagnostic is what the user sees; the returned error drives `?`-based short-circuiting of
/// the rest of the offending expression.
fn report(
    diagnostics: &mut DiagnosticBag,
    message: String,
    span: Option<TextSpan>,
) -> SemanticError {
    diagnostics.report_error(message.clone(), span);
    SemanticError::reported(message, span)
}

/// An empty source span, used for diagnostics on synthesized nodes that have no real
/// position in the user's source (e.g. array element type mismatches).
fn empty_span() -> TextSpan {
    TextSpan::new((0, 0), &Rc::new(LineText::new(String::new())))
}

/// Creates a token with an empty source span, used when the analyzer synthesizes
/// AST nodes (injected `this` parameters, monomorphized generic types, etc.).
fn synthetic_token(kind: TokenKind, text: &str) -> SyntaxToken {
    SyntaxToken::new(kind, empty_span(), text.to_string())
}

/// Builds the generic substitution bindings (parameter name -> concrete type name) by
/// zipping declared generic parameters with the supplied concrete arguments. Extra
/// parameters or arguments beyond the common length are ignored (arity is validated
/// separately so a clear diagnostic is produced).
pub(crate) fn generic_bindings(params: &[SyntaxToken], args: &[Type]) -> GenericBindings {
    params
        .iter()
        .zip(args.iter())
        .map(|(param, arg)| (param.text.clone(), arg.clone()))
        .collect()
}

/// Looks up the concrete type bound to a generic parameter name, if any.
fn lookup_binding(bindings: &GenericBindings, name: &str) -> Option<Type> {
    bindings.get(name).cloned()
}

/// Builds a mangled function name by appending each concrete type from the bindings in order,
/// e.g. base `swap` with bindings `[(T,int),(V,string)]` becomes `swap_int_string`. The mangled
/// spelling is a WASM-symbol concern, so the concrete `Type`s are stringified only here.
fn mangle_bindings(base: &str, bindings: &GenericBindings) -> String {
    mangle_with_suffixes(base, bindings.values().map(|concrete| concrete.get_type()))
}

/// Rewrites a field type token that refers to a generic parameter (e.g. `T`, `T[]`, `T?`)
/// into its concrete form, preserving the array/nullable suffix. Tokens that do not name a
/// generic parameter are returned unchanged.
fn substitute_generic_token(token: &SyntaxToken, bindings: &GenericBindings) -> SyntaxToken {
    let mut result = token.clone();
    let (base, suffix) = if let Some(base) = token.text.strip_suffix("[]") {
        (base, "[]")
    } else if let Some(base) = token.text.strip_suffix('?') {
        (base, "?")
    } else {
        (token.text.as_str(), "")
    };
    if let Some(concrete) = lookup_binding(bindings, base) {
        result.text = format!("{}{}", concrete.get_type(), suffix);
    }
    result
}

/// Rewrites a structured field type, substituting any generic parameter that appears in it with
/// its bound concrete type. Unlike `substitute_generic_token` (which only understands `T`, `T[]`,
/// `T?` on a flat token), this recurses through arrays, nullables, generic arguments, and function
/// types, so a field like `List<T>` becomes `List<JsonValue>` rather than being flattened.
pub(crate) fn substitute_generic_type(ty: &Type, bindings: &GenericBindings) -> Type {
    match ty {
        Type::Array(inner) => Type::Array(Box::new(substitute_generic_type(inner, bindings))),
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute_generic_type(inner, bindings))),
        Type::Function(params, ret) => Type::Function(
            params
                .iter()
                .map(|p| substitute_generic_type(p, bindings))
                .collect(),
            Box::new(substitute_generic_type(ret, bindings)),
        ),
        Type::Generic(name) => bind_concrete(name, bindings).unwrap_or_else(|| ty.clone()),
        Type::Struct(token, args) => {
            // A bare struct whose name is itself a generic parameter (the common `T` case, since
            // unknown identifiers parse as `Type::Struct`).
            if args.is_none() {
                if let Some(concrete) = bind_concrete(&token.text, bindings) {
                    return concrete;
                }
            }
            let new_args = args.as_ref().map(|a| {
                a.iter()
                    .map(|x| substitute_generic_type(x, bindings))
                    .collect()
            });
            Type::Struct(token.clone(), new_args)
        }
        other => other.clone(),
    }
}

/// Resolves a generic parameter name to its bound concrete `Type`, or `None` if `name` is not a
/// bound generic parameter. Bindings now carry the concrete `Type` directly, so no reparse is needed.
fn bind_concrete(name: &str, bindings: &GenericBindings) -> Option<Type> {
    bindings.get(name).cloned()
}

/// Extracts the declared generic parameter names (`["T", "V"]`) from an optional parameter-token
/// list, for registering a nominal def's arity in the [`TypeCtx`].
fn generic_param_names(params: &Option<Vec<SyntaxToken>>) -> Vec<String> {
    params
        .as_deref()
        .map(|ps| ps.iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default()
}

/// The internal member name a property getter is registered under. The `$` cannot appear in a
/// source identifier, so this never collides with a user method (including the indexer `get`) and
/// is not directly callable as `obj.get$prop()`.
pub(super) fn getter_member_name(prop: &str) -> String {
    format!("get${}", prop)
}

/// The internal member name a property setter is registered under (see [`getter_member_name`]).
pub(super) fn setter_member_name(prop: &str) -> String {
    format!("set${}", prop)
}

/// The internal member name a class member is registered under: the `$`-tagged accessor name for a
/// property `get`/`set`, or the plain method/field name otherwise.
pub(super) fn accessor_member_name(method: &FunctionNode) -> String {
    match method.accessor {
        Some(crate::syntax::nodes::function::AccessorKind::Get) => {
            getter_member_name(&method.name.text)
        }
        Some(crate::syntax::nodes::function::AccessorKind::Set) => {
            setter_member_name(&method.name.text)
        }
        None => method.name.text.clone(),
    }
}

/// Maps each generic parameter name to the concrete `Type` bound to it for one monomorphization.
/// Insertion-ordered so the mangled instance symbol (built from the values in order) is
/// deterministic. Stores the structured AST `Type` (not a stringified name), so the monomorphizer
/// substitutes and lowers it directly rather than round-tripping through `get_type()`/reparse.
pub type GenericBindings = IndexMap<String, Type>;

/// Enum name -> (member name -> integer value). Insertion-ordered at both levels so the enum
/// variant-name interning that feeds emitted output happens in a deterministic (declaration) order.
pub type EnumTable = IndexMap<String, IndexMap<String, i32>>;

/// A resolved top-level variable, carried from semantic analysis into code generation so the
/// generator can emit the corresponding WASM global and the module-init store (and decide whether
/// to export it to the host).
#[derive(Debug, Clone)]
pub struct GlobalSymbol {
    pub name: String,
    /// The resolved (non-generic) type name, e.g. `int`, `string`, `Point`.
    pub type_str: String,
    pub is_const: bool,
    pub is_public: bool,
    pub is_static: bool,
}

pub struct SemanticInfo<'a> {
    pub hash_map: HashMap<String, Rc<RefCell<SymbolTable>>>,
    pub function_table: &'a FunctionTable,
    pub struct_table: &'a StructTable,
    pub instantiated_generics: IndexMap<String, (GenericBindings, &'a FunctionNode<'a>)>,
    pub struct_methods: Vec<(&'a FunctionNode<'a>, GenericBindings)>,
    pub enums: EnumTable,
    /// Layout of every (monomorphized) discriminated union, surfaced to codegen so it can
    /// allocate variant blocks, lower `match`, and emit discriminant-aware releases.
    pub unions: UnionTable,
    pub globals: Vec<GlobalSymbol>,
    /// The typed, name-resolved HIR emitted alongside analysis. It is the sole input the MIR backend
    /// consumes; a function whose every construct is representable is emitted here (all others are
    /// skipped and produce no backend output).
    pub hir: crate::hir::Hir,
}

/// Groups context arguments frequently passed together to simplify function signatures.
pub struct AnalyzerContext<'a, 'b> {
    pub parent_function: &'b FunctionNode<'a>,
    pub symbol_table: &'b Rc<RefCell<SymbolTable>>,
}

pub struct Analyzer<'a> {
    syntax_tree: &'a SyntaxTree<'a>,
    function_table: FunctionTable,
    struct_table: StructTable,
    arena: &'a Bump,
    generic_functions: HashMap<String, &'a FunctionNode<'a>>,
    instantiated_generics: IndexMap<String, (GenericBindings, &'a FunctionNode<'a>)>,
    generic_structs:
        HashMap<String, &'a crate::syntax::nodes::struct_node::StructDeclarationNode<'a>>,
    struct_methods: Vec<(&'a FunctionNode<'a>, GenericBindings)>,
    /// Registered enums: name -> (member -> value). Enum values are plain `i32`s at runtime.
    enum_table: EnumTable,
    /// Layout of every registered (monomorphized) discriminated union.
    union_table: UnionTable,
    /// Generic discriminated-union templates (`enum Option<T> { ... }`), instantiated on demand.
    generic_unions: HashMap<String, &'a EnumDeclarationNode>,
    /// Generic `extend Type<...> { ... }` templates (e.g. `extend Option<T> { ... }`), keyed by
    /// the extended type's name. Their methods are monomorphized alongside each concrete
    /// instantiation of the target generic union or struct (see `ensure_*_instantiated`).
    generic_extends: HashMap<String, Vec<&'a ExtendNode<'a>>>,
    /// Interface name -> its method signatures in declaration order (the order is the interface's
    /// local method index, used for itable slot assignment). Each entry is a body-less
    /// [`FunctionNode`] (no implicit `this`).
    interface_methods: IndexMap<String, Vec<&'a FunctionNode<'a>>>,
    /// Generic interface templates (`interface Container<T> { ... }`), instantiated on demand into
    /// concrete `interface_methods` entries (e.g. `Container_int`) — mirrors `generic_structs`.
    generic_interfaces: HashMap<String, &'a crate::syntax::nodes::InterfaceDeclarationNode<'a>>,
    /// Class name -> the interfaces it implements (in `class C : A, B` order), recorded after the
    /// implements clause is validated. Names are mangled for generic instances (e.g. `Box_int` ->
    /// `Container_int`). Drives interface-typed assignability and itable emission.
    implements: HashMap<String, Vec<String>>,
    /// Names of types declared `sealed` (class/struct/enum). A user `extend` block may not target
    /// any of these; compiler-synthesized extends (interface defaults) are exempt.
    sealed_types: std::collections::HashSet<String>,
    /// An optional expected type for the expression currently being analyzed (from a `let`
    /// annotation or `return` type). Used to resolve the type arguments of a generic union's
    /// nullary variant (`let o: Option<int> = Option.None;`), where they cannot be inferred from
    /// arguments. `None` outside such contexts.
    current_expected_type: Option<Type>,
    /// The generic substitution bindings active while analyzing a monomorphized function or
    /// struct-method body. Empty outside of any generic instantiation. Used to resolve generic
    /// type parameters that appear inside a body (e.g. the `T` in `array_new<T>(...)`).
    current_generic_bindings: GenericBindings,
    /// Stack of loop labels currently in scope, so `break label;`/`continue label;` can be
    /// validated against an enclosing labeled loop.
    loop_labels: Vec<String>,
    /// Label attached to the immediately-following loop (`outer: for ...`), consumed by that loop's
    /// analyzer so it can be threaded into the loop's HIR node. `None` for unlabeled loops.
    pending_loop_label: Option<String>,
    /// True while analyzing the body of an `async fun`. Gates the use of `await`.
    current_function_is_async: bool,
    /// Resolved top-level variables, in declaration order. Surfaced to codegen via [`SemanticInfo`].
    globals: Vec<GlobalSymbol>,
    /// The module-level symbol scope holding every top-level variable. It is the root parent of
    /// every function's parameter table, so function bodies resolve global identifiers (and their
    /// `const`-ness) through ordinary lexical lookup.
    global_symbol_table: Rc<RefCell<SymbolTable>>,
    /// The structured type context (interner + def table). Nominal declarations register their
    /// `DefId` here and AST type annotations lower to interned `TypeId`s, so type identity,
    /// compatibility, and monomorphization keys move off strings onto the structured type system.
    type_ctx: TypeCtx,
    /// Interleaved HIR-emission state and the accumulated emitted functions.
    hir: hir_emit::HirEmit,
}
impl<'a> Analyzer<'a> {
    pub fn new(tree: &'a SyntaxTree<'a>, arena: &'a Bump) -> Self {
        Self {
            syntax_tree: tree,
            function_table: FunctionTable::new(),
            struct_table: StructTable::new(),
            arena,
            generic_functions: HashMap::new(),
            instantiated_generics: IndexMap::new(),
            generic_structs: HashMap::new(),
            struct_methods: Vec::new(),
            enum_table: IndexMap::new(),
            union_table: IndexMap::new(),
            generic_unions: HashMap::new(),
            generic_extends: HashMap::new(),
            interface_methods: IndexMap::new(),
            generic_interfaces: HashMap::new(),
            sealed_types: std::collections::HashSet::new(),
            implements: HashMap::new(),
            current_expected_type: None,
            current_generic_bindings: GenericBindings::new(),
            loop_labels: Vec::new(),
            pending_loop_label: None,
            current_function_is_async: false,
            globals: Vec::new(),
            global_symbol_table: Rc::new(RefCell::new(SymbolTable::new(None))),
            type_ctx: TypeCtx::new(),
            hir: hir_emit::HirEmit::default(),
        }
    }

    /// The type interner backing analysis. Its `TypeId`s are the ones referenced by the emitted HIR
    /// (`SemanticInfo::hir`), so the MIR backend must be handed *this* interner to lower that HIR.
    pub(crate) fn interner(&self) -> &crate::types::TypeInterner {
        &self.type_ctx.interner
    }

    /// Builds the `Future<T>` type carrying inner type `inner`. Async-call results are this type,
    /// and `await` unwraps it back to `inner`.
    pub(super) fn future_type(inner: Type) -> Type {
        Type::Struct(
            synthetic_token(TokenKind::IdentifierToken, FUTURE_TYPE),
            Some(vec![inner]),
        )
    }

    /// If `ty` is a `Future<T>`, returns the inner `T`; otherwise `None`.
    pub(super) fn future_inner_type(ty: &Type) -> Option<Type> {
        match ty {
            Type::Struct(token, Some(args)) if token.text == FUTURE_TYPE && args.len() == 1 => {
                Some(args[0].clone())
            }
            _ => None,
        }
    }
    pub fn analyze(
        &mut self,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<SemanticInfo<'_>, SemanticError> {
        let pgm = self.syntax_tree.get_root();
        self.analyze_pgm(pgm, diagnostics)
    }

    /// Runs `f` with `current_generic_bindings` set to `bindings`, restoring the previous bindings
    /// afterward (even if `f` returns early via `?`). Replaces the manual "set then clear to empty"
    /// pattern at the monomorphized-body analysis sites, which both leaked bindings into the next
    /// body on an error path and clobbered (rather than restored) any enclosing bindings.
    pub(super) fn with_generic_bindings<F, R>(&mut self, bindings: GenericBindings, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        let saved = std::mem::replace(&mut self.current_generic_bindings, bindings);
        let result = f(self);
        self.current_generic_bindings = saved;
        result
    }

    /// Runs `f` with `current_function_is_async` set to `is_async`, restoring the previous value
    /// afterward so the flag cannot leak into a sibling function's analysis.
    pub(super) fn with_async_flag<F, R>(&mut self, is_async: bool, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        let saved = self.current_function_is_async;
        self.current_function_is_async = is_async;
        let result = f(self);
        self.current_function_is_async = saved;
        result
    }

    /// Builds a concrete `Type` from a type name, used when substituting a generic
    /// parameter `T` with the concrete type chosen at the call/instantiation site.
    fn concrete_type_from_str(name: &str) -> Type {
        let token = synthetic_token(TokenKind::DataTypeToken, name);
        primitive_type(name, token.clone()).unwrap_or(Type::Struct(token, None))
    }

    /// If `ty` is a struct (or nullable struct), returns its base name and the list of
    /// concrete generic type arguments (empty for non-generic structs). Returns `None`
    /// for any non-struct type. Does NOT recurse into arrays (a method/member access on an
    /// array is invalid and must surface as an error).
    fn resolve_struct_parts(ty: &Type) -> Option<(String, Vec<Type>)> {
        match ty {
            Type::Struct(token, args) => {
                Some((token.text.clone(), args.clone().unwrap_or_default()))
            }
            Type::Nullable(inner) => Self::resolve_struct_parts(inner),
            _ => None,
        }
    }
    fn analyze_pgm(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<SemanticInfo<'_>, SemanticError> {
        let mut symbol_table_map = HashMap::new();

        // Stash generic `extend` templates before any type instantiation can occur (a concrete
        // union/struct field may instantiate a generic union during `register_enums`), so the
        // extension methods are always available to attach at the first instantiation.
        self.stash_generic_extensions(node);
        self.register_enums(node, diagnostics);
        // Interfaces are registered before structs so a class's implements clause can be validated
        // against the interface method signatures during struct registration.
        self.register_interfaces(node, diagnostics);
        self.register_structs(node, diagnostics);
        self.register_extensions(node, diagnostics);
        self.register_functions(node, diagnostics);
        // Globals are analyzed after functions/types are known (so initializers can call them) but
        // before function bodies, so those bodies can resolve global identifiers.
        // HIR global slots are assigned incrementally inside `register_globals` (in declaration
        // order) so both later initializers and function bodies can resolve global identifiers.
        self.register_globals(node, diagnostics);
        self.analyze_function_bodies(node, &mut symbol_table_map, diagnostics)?;
        self.analyze_pending_instantiations(&mut symbol_table_map, diagnostics)?;

        // Per-statement/expression analysis recovers locally (reporting into the bag and poisoning
        // with `Type::Unknown`) so every independent error in the program is surfaced. The typed
        // boundary failure is raised once here, from the aggregate error state, so the driver can
        // abort before code generation.
        if diagnostics.has_errors() {
            return Err(SemanticError::AnalysisFailed);
        }

        // Built before the borrow-immutable `SemanticInfo` literal below, since lowering field types
        // needs `&mut self.type_ctx`.
        let layouts = self.hir_build_layouts();
        let imports = self.hir_build_imports(node);
        let intrinsics = self.hir_build_intrinsics(node);
        let interfaces = self.hir_build_interfaces();
        let hir_functions = std::mem::take(&mut self.hir.functions);
        let hir_globals = std::mem::take(&mut self.hir.global_decls);

        Ok(SemanticInfo {
            hash_map: symbol_table_map,
            function_table: &self.function_table,
            struct_table: &self.struct_table,
            instantiated_generics: self.instantiated_generics.clone(),
            struct_methods: self.struct_methods.clone(),
            enums: self.enum_table.clone(),
            unions: self.union_table.clone(),
            globals: self.globals.clone(),
            hir: crate::hir::Hir {
                functions: hir_functions,
                globals: hir_globals,
                instances: vec![],
                layouts,
                imports,
                intrinsics,
                interfaces,
            },
        })
    }
}

#[cfg(test)]
#[path = "../tests/mod.rs"]
mod tests;
