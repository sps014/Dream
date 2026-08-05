use crate::semantics::errors::SymbolError;
use crate::stdlib::StdlibFunction;
use crate::syntax::nodes::{FunctionNode, Type, Visibility};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct FunctionTable {
    pub functions: HashMap<String, FunctionTableInfo>,
    /// Overload namespace -> the emitted keys of every overload registered under it, in
    /// declaration order. The namespace is the bare base when no cross-module collision occurred,
    /// or `module::base` after promotion (see [`module_key`]). A namespace with a single entry
    /// keeps that name as-is; a namespace with 2+ entries stores each under a signature-mangled
    /// key (see [`overload_key`]).
    pub overloads: HashMap<String, Vec<String>>,
    /// (declaring module, base name) -> the overload *namespace* for that module's declarations
    /// of `base`, once a same-name collision across two *different* declared modules has
    /// promoted both to module-qualified namespaces (see [`Self::add_overload`]/[`module_key`]).
    /// The value is `module::base` — overload mangling then appends `.TypeId…` onto that
    /// namespace. Absent entries mean "no cross-module collision for this name": look it up by its
    /// bare name instead. Never populated for the unnamed root module (`None`), so unmoded code
    /// keeps today's flat "same bare name always collides" behavior untouched.
    by_module: HashMap<(Option<Rc<str>>, String), String>,
}

/// Builds the overload namespace for a declaration named `base` once a same-name collision with a
/// *different* declared module has forced both onto module-qualified namespaces, e.g. base `add`
/// in module `utils.math` becomes `utils.math::add`. Overload mangling composes on top via
/// [`overload_key`] (e.g. `utils.math::add.0.1`). `::` is a valid WAT identifier character
/// disjoint from the `.` [`overload_key`] uses, so the two mangling schemes compose safely.
fn module_key(base: &str, module: Option<&str>) -> String {
    match module {
        Some(m) => format!("{m}::{base}"),
        None => base.to_string(),
    }
}

/// Result of resolving an overloaded call against the argument types present at a call site.
pub enum OverloadResolution {
    Unique(String),
    None,
    Ambiguous(Vec<String>),
}

/// Builds the signature-mangled emitted name for one overload: the overload namespace followed by
/// each parameter's interned [`TypeId`] (decimal), joined with `.` — a valid WAT identifier
/// character, distinct from the `_` used by generic monomorphization so the two schemes never
/// collide. E.g. namespace `add` with two params whose TypeIds are 0 and 0 becomes `add.0.0`; a
/// zero-parameter overload becomes `add.`. When the namespace is already module-qualified the
/// same rule composes: `utils.math::add.0.0`.
pub fn overload_key(
    base: &str,
    parameters: &[String],
    type_ctx: &mut crate::types::TypeCtx,
) -> String {
    let mut key = String::from(base);
    key.push('.');
    let mut parts = Vec::new();
    for p in parameters {
        parts.push(type_ctx.lower_str(p).0.to_string());
    }
    key.push_str(&parts.join("."));
    key
}

impl Default for FunctionTable {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionTable {
    pub fn new() -> FunctionTable {
        let mut table = FunctionTable {
            functions: HashMap::new(),
            overloads: HashMap::new(),
            by_module: HashMap::new(),
        };

        for std_func in StdlibFunction::get_all() {
            let info = FunctionTableInfo::new(
                std_func.name.clone(),
                std_func.return_type,
                std_func.parameters,
            );
            table.functions.insert(std_func.name, info);
        }

        table
    }

    pub fn add_function(
        &mut self,
        name: String,
        function_info: FunctionTableInfo,
    ) -> Result<(), SymbolError> {
        if self.functions.contains_key(&name) {
            return Err(SymbolError::new(format!(
                "Function already exists ({})",
                name
            )));
        }
        self.functions.insert(name, function_info);
        Ok(())
    }

    /// Registers one (possibly overloaded) declaration under `base`. The first declaration of a
    /// namespace keeps that name; when a second declaration arrives the original is *promoted* to
    /// its signature-mangled key and the new one is mangled too, so non-overloaded code keeps its
    /// original emitted names. A same-named declaration from a *different* declared module is not
    /// an overload conflict — both sides are promoted to module-qualified namespaces
    /// (`module::base`), and overload mangling composes on top (`module::base.TypeId…`). Returns
    /// the emitted key chosen for `info`, or an error if an identical signature was already
    /// registered under the resolved namespace.
    pub fn add_overload(
        &mut self,
        base: &str,
        info: FunctionTableInfo,
        type_ctx: &mut crate::types::TypeCtx,
    ) -> Result<String, SymbolError> {
        if info.declaring_module.is_some() && self.needs_promote_bare(base, &info.declaring_module) {
            self.promote_bare_to_modules(base, type_ctx)?;
        }

        let ns = self.namespace_for(base, info.declaring_module.as_ref());
        if ns != base {
            self.by_module.insert(
                (info.declaring_module.clone(), base.to_string()),
                ns.clone(),
            );
        }
        self.register_under_namespace(&ns, info, type_ctx, base)
    }

    /// True when `base` is still held under its bare name/overload set by a *different* declared
    /// module than `incoming`, so the bare entries must be rewritten to module-qualified
    /// namespaces before `incoming` can register.
    fn needs_promote_bare(&self, base: &str, incoming: &Option<Rc<str>>) -> bool {
        // Already recorded under a module-qualified namespace for this module — nothing bare left
        // to promote for the incoming side.
        if self
            .by_module
            .contains_key(&(incoming.clone(), base.to_string()))
        {
            return false;
        }
        matches!(
            self.bare_holder_module(base),
            Some(holder) if holder.is_some() && holder != *incoming
        )
    }

    /// The declaring module of whatever currently owns the bare `base` namespace (singleton or
    /// overload set), if any.
    fn bare_holder_module(&self, base: &str) -> Option<Option<Rc<str>>> {
        if let Some(info) = self.functions.get(base) {
            return Some(info.declaring_module.clone());
        }
        let keys = self.overloads.get(base)?;
        let first = keys.first()?;
        self.functions
            .get(first)
            .map(|info| info.declaring_module.clone())
    }

    /// Overload namespace for `base` as declared in `module`: the module-qualified namespace when a
    /// cross-module collision has forced qualification, otherwise the bare base.
    fn namespace_for(&self, base: &str, module: Option<&Rc<str>>) -> String {
        if let Some(ns) = self.resolve_in_module(module, base) {
            return ns.to_string();
        }
        // Another module already forced qualification for this bare name — this module must
        // qualify too, even if it has not yet recorded a by_module entry.
        if self.by_module.keys().any(|(_, n)| n == base) {
            return module_key(base, module.map(|m| m.as_ref()));
        }
        base.to_string()
    }

    /// Rewrites every declaration currently stored under the bare `base` namespace into
    /// module-qualified namespaces (`module::base` / `module::base.TypeId…`), recording each in
    /// [`Self::by_module`]. Preserves per-module declaration order.
    fn promote_bare_to_modules(
        &mut self,
        base: &str,
        type_ctx: &mut crate::types::TypeCtx,
    ) -> Result<(), SymbolError> {
        let keys: Vec<String> = match self.overloads.remove(base) {
            Some(keys) => keys,
            None if self.functions.contains_key(base) => vec![base.to_string()],
            None => return Ok(()),
        };
        let mut infos = Vec::with_capacity(keys.len());
        for key in &keys {
            if let Some(info) = self.functions.remove(key) {
                infos.push(info);
            }
        }

        // Group by declaring module while preserving first-seen module order and per-group
        // declaration order (HashMap iteration must not decide emit-facing order).
        let mut module_order: Vec<Option<Rc<str>>> = Vec::new();
        let mut groups: HashMap<Option<Rc<str>>, Vec<FunctionTableInfo>> = HashMap::new();
        for info in infos {
            let module = info.declaring_module.clone();
            if !groups.contains_key(&module) {
                module_order.push(module.clone());
            }
            groups.entry(module).or_default().push(info);
        }

        for module in module_order {
            let group = groups.remove(&module).unwrap_or_default();
            let ns = module_key(base, module.as_deref());
            self.by_module
                .insert((module, base.to_string()), ns.clone());
            self.insert_group_under_namespace(&ns, group, type_ctx, base)?;
        }
        Ok(())
    }

    /// Inserts an already-collected same-module group under `ns` (singleton keeps `ns`; 2+
    /// entries are signature-mangled).
    fn insert_group_under_namespace(
        &mut self,
        ns: &str,
        group: Vec<FunctionTableInfo>,
        type_ctx: &mut crate::types::TypeCtx,
        error_name: &str,
    ) -> Result<(), SymbolError> {
        if group.is_empty() {
            return Ok(());
        }
        if group.len() == 1 {
            let mut info = group.into_iter().next().unwrap();
            if self.functions.contains_key(ns) {
                return Err(SymbolError::new(format!(
                    "Function '{}' is already defined in module '{}'",
                    error_name,
                    info.declaring_module.as_deref().unwrap_or("")
                )));
            }
            info.name = ns.to_string();
            self.overloads
                .insert(ns.to_string(), vec![ns.to_string()]);
            self.functions.insert(ns.to_string(), info);
            return Ok(());
        }
        let mut keys = Vec::with_capacity(group.len());
        for mut info in group {
            let key = overload_key(ns, &info.parameters, type_ctx);
            if self.functions.contains_key(&key) {
                return Err(SymbolError::new(format!(
                    "Duplicate overload: '{}' with the same parameter types is already defined",
                    error_name
                )));
            }
            info.name = key.clone();
            keys.push(key.clone());
            self.functions.insert(key, info);
        }
        self.overloads.insert(ns.to_string(), keys);
        Ok(())
    }

    /// Registers `info` under overload namespace `ns`, promoting a singleton to a signature-mangled
    /// key when a second overload arrives. Default parameter values are allowed; overload
    /// resolution ([`select_overload`]) prefers an exact-arity match over one that fills defaults.
    fn register_under_namespace(
        &mut self,
        ns: &str,
        mut info: FunctionTableInfo,
        type_ctx: &mut crate::types::TypeCtx,
        error_name: &str,
    ) -> Result<String, SymbolError> {
        let existing = self.overloads.entry(ns.to_string()).or_default();
        if existing.is_empty() {
            if self.functions.contains_key(ns) {
                return Err(SymbolError::new(format!(
                    "Function already exists ({})",
                    error_name
                )));
            }
            info.name = ns.to_string();
            existing.push(ns.to_string());
            self.functions.insert(ns.to_string(), info);
            return Ok(ns.to_string());
        }
        // Promote a lone singleton to its mangled key the moment a second overload appears.
        if existing.len() == 1 && existing[0] == ns {
            if let Some(mut first) = self.functions.remove(ns) {
                let first_key = overload_key(ns, &first.parameters, type_ctx);
                first.name = first_key.clone();
                self.functions.insert(first_key.clone(), first);
                existing[0] = first_key;
            }
        }
        let key = overload_key(ns, &info.parameters, type_ctx);
        if self.functions.contains_key(&key) {
            return Err(SymbolError::new(format!(
                "Duplicate overload: '{}' with the same parameter types is already defined",
                error_name
            )));
        }
        info.name = key.clone();
        existing.push(key.clone());
        self.functions.insert(key.clone(), info);
        Ok(key)
    }

    /// The overload namespace registered for `base` as declared in `module`, if a cross-module
    /// collision ever forced it onto a module-qualified namespace. Returns `None` when no such
    /// collision occurred for this name (the common case): callers should then fall back to
    /// looking `base` up by its bare name, unchanged from before `module` existed.
    pub fn resolve_in_module(&self, module: Option<&Rc<str>>, base: &str) -> Option<&str> {
        self.by_module
            .get(&(module.cloned(), base.to_string()))
            .map(|s| s.as_str())
    }

    /// Resolves `item` as declared in `module_path` to its overload namespace (module-qualified
    /// after a cross-module collision, otherwise the bare name when that bare namespace belongs to
    /// the requested module). Used by aliased imports, including overloaded items whose functions
    /// table entries live under signature-mangled keys rather than the namespace itself.
    pub fn resolve_item_namespace(&self, module_path: &str, item: &str) -> Option<String> {
        let module: Rc<str> = Rc::from(module_path);
        if let Some(ns) = self.resolve_in_module(Some(&module), item) {
            return Some(ns.to_string());
        }
        if let Some(keys) = self.overloads.get(item) {
            let first = keys.first()?;
            let info = self.functions.get(first)?;
            if info.declaring_module.as_deref() == Some(module_path) {
                return Some(item.to_string());
            }
            return None;
        }
        self.functions
            .get(item)
            .filter(|info| info.declaring_module.as_deref() == Some(module_path))
            .map(|info| info.name.clone())
    }

    /// Whether `base` has more than one overload (i.e. its declarations are signature-mangled).
    pub fn is_overloaded(&self, base: &str) -> bool {
        self.overloads
            .get(base)
            .map(|v| v.len() > 1)
            .unwrap_or(false)
    }

    /// The emitted name of the declaration of `base` whose parameter list is `parameters`: the
    /// bare base when `base` is not overloaded, otherwise the signature-mangled key.
    pub fn resolve_emitted_name(
        &self,
        base: &str,
        parameters: &[String],
        type_ctx: &mut crate::types::TypeCtx,
    ) -> String {
        if self.is_overloaded(base) {
            overload_key(base, parameters, type_ctx)
        } else {
            base.to_string()
        }
    }

    /// The emitted name of the declaration of `base` as declared in `module`: when a cross-module
    /// collision promoted it, resolves against the module-qualified overload namespace (applying
    /// signature mangling when that namespace is overloaded); otherwise
    /// [`Self::resolve_emitted_name`]'s ordinary bare-name/overload-mangled result.
    pub fn resolve_emitted_name_scoped(
        &self,
        base: &str,
        module: Option<&Rc<str>>,
        parameters: &[String],
        type_ctx: &mut crate::types::TypeCtx,
    ) -> String {
        if let Some(ns) = self.resolve_in_module(module, base) {
            return self.resolve_emitted_name(ns, parameters, type_ctx);
        }
        self.resolve_emitted_name(base, parameters, type_ctx)
    }

    /// Selects the overload of `base` that best matches `args`. Exact type matches are preferred;
    /// `compat` supplies the fallback compatibility (object widening, enum/int, numeric, nullable).
    /// A single best candidate wins; ties yield `Ambiguous`; no viable candidate yields `None`.
    /// When `base` is not an overload set, falls back to the plain function keyed by `base`.
    pub fn select_overload(
        &self,
        base: &str,
        args: &[String],
        mut compat: impl FnMut(&str, &str) -> bool,
    ) -> OverloadResolution {
        let keys = match self.overloads.get(base) {
            Some(keys) => keys,
            None => {
                return if self.functions.contains_key(base) {
                    OverloadResolution::Unique(base.to_string())
                } else {
                    OverloadResolution::None
                };
            }
        };
        let mut scored: Vec<(i32, &String)> = Vec::new();
        for key in keys {
            let info = match self.functions.get(key) {
                Some(info) => info,
                None => continue,
            };
            // A defaulted overload matches any argument count from its required count up to its
            // full arity; the omitted trailing parameters are filled from their defaults later.
            if args.len() < info.required_params() || args.len() > info.parameters.len() {
                continue;
            }
            let mut score = 0i32;
            let mut viable = true;
            // Only the supplied arguments are type-checked against their parameters; defaulted
            // trailing parameters are guaranteed to match their own literal defaults.
            for (param, arg) in info.parameters.iter().zip(args.iter()) {
                if param == arg {
                    score += 1;
                } else if compat(param, arg) {
                    // Viable via fallback (e.g. object widening); contributes no exactness score.
                } else {
                    viable = false;
                    break;
                }
            }
            // Prefer an overload whose arity exactly matches the call (no defaults filled) over one
            // that relies on defaults, so `f(int)` beats `f(int, int = 0)` for a one-argument call.
            if viable && args.len() == info.parameters.len() {
                score += 1;
            }
            if viable {
                scored.push((score, key));
            }
        }
        let max_score = match scored.iter().map(|(s, _)| *s).max() {
            Some(max) => max,
            None => return OverloadResolution::None,
        };
        let best: Vec<String> = scored
            .iter()
            .filter(|(s, _)| *s == max_score)
            .map(|(_, k)| (*k).clone())
            .collect();
        if best.len() == 1 {
            OverloadResolution::Unique(best.into_iter().next().unwrap())
        } else {
            OverloadResolution::Ambiguous(best)
        }
    }

    pub fn get_function(&self, name: &String) -> Result<FunctionTableInfo, SymbolError> {
        if !self.functions.contains_key(name) {
            return Err(SymbolError::new(format!(
                "Function does not exist ({})",
                name
            )));
        }
        Ok(self.functions.get(name).unwrap().clone())
    }
}

#[derive(Debug, Clone)]
pub struct FunctionTableInfo {
    pub name: String,
    pub return_type: Option<Type>,
    pub parameters: Vec<String>,
    /// The fully structured (never string-mangled) counterpart of `parameters`, parallel to it.
    /// Populated by [`FunctionTableInfo::from`] straight from the declaration's (possibly
    /// generic-substituted) `ParameterNode::type_`, so a generic-struct-typed parameter (e.g.
    /// `List<T>`, concretized to `List<int>` on a monomorphized method) keeps its `Struct(name,
    /// Some(args))` shape instead of collapsing to the opaque mangled name `parameters` stores.
    /// Used to publish `current_expected_type` per call argument (see `analyze_call_arguments_expecting`),
    /// which a string round-trip through `parameters` cannot do losslessly for generic structs.
    /// Empty for synthesized/stdlib entries built via [`FunctionTableInfo::new`] (host functions
    /// only ever take primitive parameters, so the string form round-trips losslessly there).
    pub parameter_types: Vec<Type>,
    /// Per-parameter declared names, parallel to `parameters`, used to resolve named arguments
    /// (`f(name: value)`) at call sites back to a positional index. Empty for entries with no
    /// source-level parameter names (synthesized/stdlib entries built via
    /// [`FunctionTableInfo::new`]) — a named-argument call to one of those is rejected with a clear
    /// diagnostic rather than silently misresolving.
    pub param_names: Vec<String>,
    /// True when the last declared parameter is `...name: T[]` (variadic): a call may supply zero
    /// or more trailing arguments of the array's element type in that slot, which the analyzer
    /// collects into an array before argument type-checking. `false` for every synthesized/stdlib
    /// entry and every declaration with no variadic parameter.
    pub is_variadic: bool,
    /// Per-parameter `ref` flag, parallel to `parameters`: true when the declaration is `ref
    /// name: T`, requiring the call site to pass a matching `ref` argument (see
    /// `Analyzer::analyze_ref_argument`). Always all-`false` for synthesized/stdlib entries.
    pub is_ref: Vec<bool>,
    /// Per-parameter constant-literal default values, parallel to `parameters`. `None` means the
    /// parameter is required. Defaults are always trailing (enforced by the parser), so a call may
    /// omit the trailing defaulted arguments and the analyzer substitutes these literals.
    pub defaults: Vec<Option<Type>>,
    /// True when the declaration is `async fun`: calling it eagerly starts a task and yields
    /// `Future<T>` (where `T` is `return_type`). Awaiting a call to it produces `T`.
    pub is_async: bool,
    /// True when the declaration is a `static fun` method (no implicit `this`, dispatched as
    /// `Type.method(...)`). Used by the indexer/enumerator sugar sites to reject static methods as
    /// `[]`/`for..in` hooks. Always `false` for free functions and synthesized/stdlib entries.
    pub is_static: bool,
    /// True when the declaration carries `@unsafe`: it performs a manual-memory-management
    /// operation (raw `Pointer<T>` alloc/free/realloc/read/write) with no compiler-enforced safety
    /// net. Calling it is only permitted from another `@unsafe` function/method — checked at every
    /// call site (see `Analyzer::check_unsafe_call`, `src/semantics/analyzer/calls/mod.rs`).
    pub is_unsafe: bool,
    pub intrinsic_name: Option<String>,
    /// Accessibility of the declaration. For methods this gates external calls (private methods
    /// may only be called from within their declaring type; `internal` ones from anywhere in the
    /// same module). Defaults to `Public` for synthesized/stdlib entries so they are callable
    /// everywhere.
    pub visibility: Visibility,
    /// Source file the declaration came from, used for file/module-level visibility: a non-public
    /// declaration is only reachable from its own file. `None` for synthesized/stdlib entries,
    /// which are always visible.
    pub declaring_file: Option<std::rc::Rc<str>>,
    /// The declaring file's `module a.b.c;` path, if any — `None` for a file with no `module`
    /// declaration (the implicit root module) as well as for synthesized/stdlib entries. Set by
    /// the analyzer's registration pass (`FunctionTableInfo::from` cannot see the file/module map
    /// on its own); drives the cross-module duplicate-name resolution in [`FunctionTable::add_overload`].
    pub declaring_module: Option<std::rc::Rc<str>>,
}

impl FunctionTableInfo {
    pub fn new(
        name: String,
        return_type: Option<Type>,
        parameters: Vec<String>,
    ) -> FunctionTableInfo {
        let defaults = vec![None; parameters.len()];
        let is_ref = vec![false; parameters.len()];
        let param_names = Vec::new();
        FunctionTableInfo {
            name,
            return_type,
            parameters,
            parameter_types: Vec::new(),
            param_names,
            is_variadic: false,
            is_ref,
            defaults,
            is_async: false,
            is_static: false,
            is_unsafe: false,
            intrinsic_name: None,
            visibility: Visibility::Public,
            declaring_file: None,
            declaring_module: None,
        }
    }
    pub fn from(func: &FunctionNode) -> Self {
        let name = func.name.clone();
        let return_type = func.return_type.clone();
        let mut parameters: Vec<String> = vec![];
        let mut parameter_types: Vec<Type> = vec![];
        let mut param_names: Vec<String> = vec![];
        let mut defaults: Vec<Option<Type>> = vec![];
        let mut is_ref: Vec<bool> = vec![];
        for i in func.parameters.iter() {
            let j = i.clone();
            parameters.push(j.type_.get_type());
            parameter_types.push(j.type_);
            param_names.push(j.name.text);
            defaults.push(j.default);
            is_ref.push(j.is_ref);
        }
        let intrinsic_name = crate::intrinsics::intrinsic_key(&func.attributes);
        let is_variadic = func
            .parameters
            .last()
            .map(|p| p.is_variadic)
            .unwrap_or(false);
        let mut info = FunctionTableInfo::new(name.text, return_type, parameters);
        info.parameter_types = parameter_types;
        info.param_names = param_names;
        info.is_variadic = is_variadic;
        info.is_ref = is_ref;
        info.defaults = defaults;
        info.is_async = func.is_async;
        info.is_static = func.is_static;
        info.is_unsafe = func.attributes.iter().any(|a| a.name.text == "unsafe");
        info.intrinsic_name = intrinsic_name;
        // `extern` functions/methods are interop entry points (WASM imports): they cannot be
        // host-exported and privacy is meaningless for them, so they are always call-visible.
        info.visibility = if func.is_extern {
            Visibility::Public
        } else {
            func.visibility
        };
        info.declaring_file = func.file_path.clone();
        info
    }

    /// The number of leading required parameters: the index of the first parameter that has a
    /// default value, or the full parameter count when none do. A call must supply at least this
    /// many arguments; the remaining trailing parameters may be omitted (their defaults are used).
    pub fn required_params(&self) -> usize {
        self.defaults
            .iter()
            .position(|d| d.is_some())
            .unwrap_or(self.parameters.len())
    }

    /// True if any parameter carries a default value.
    pub fn has_defaults(&self) -> bool {
        self.defaults.iter().any(|d| d.is_some())
    }
}
