use crate::semantics::errors::SymbolError;
use crate::stdlib::StdlibFunction;
use crate::syntax::nodes::{FunctionNode, Type};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FunctionTable {
    pub functions: HashMap<String, FunctionTableInfo>,
    /// Base name -> the emitted keys of every overload registered under it, in declaration
    /// order. A base with a single entry keeps its bare name; a base with 2+ entries has each
    /// overload stored under a signature-mangled key (see [`overload_key`]).
    pub overloads: HashMap<String, Vec<String>>,
}

/// Result of resolving an overloaded call against the argument types present at a call site.
pub enum OverloadResolution {
    Unique(String),
    None,
    Ambiguous(Vec<String>),
}

/// Builds the signature-mangled emitted name for one overload: the base name followed by each
/// parameter type, joined with `.` — a valid WAT identifier character, distinct from the `_`
/// used by generic monomorphization so the two schemes never collide. E.g. base `add` with
/// `[int, int]` becomes `add.int.int`; a zero-parameter overload becomes `add.`.
pub fn overload_key(base: &str, parameters: &[String], type_ctx: &mut crate::types::TypeCtx) -> String {
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
    /// base keeps the bare name; when a second declaration arrives the original is *promoted* to
    /// its signature-mangled key and the new one is mangled too, so non-overloaded code keeps its
    /// original emitted names. Returns the emitted key chosen for `info`, or an error if an
    /// identical signature was already registered under `base`.
    pub fn add_overload(
        &mut self,
        base: &str,
        mut info: FunctionTableInfo,
        type_ctx: &mut crate::types::TypeCtx,
    ) -> Result<String, SymbolError> {
        let existing = self.overloads.entry(base.to_string()).or_default();
        if existing.is_empty() {
            if self.functions.contains_key(base) {
                return Err(SymbolError::new(format!(
                    "Function already exists ({})",
                    base
                )));
            }
            info.name = base.to_string();
            existing.push(base.to_string());
            self.functions.insert(base.to_string(), info);
            return Ok(base.to_string());
        }
        // Default parameter values are only supported on non-overloaded functions: an omitted
        // trailing argument would make overload resolution ambiguous. Reject the combination
        // whether the default is on the new overload or on one already registered. (An explicit
        // loop rather than a closure, so it borrows only `self.functions`, disjoint from the
        // mutable `existing` borrow of `self.overloads`.)
        let mut existing_has_defaults = false;
        for k in existing.iter() {
            if self.functions.get(k).map(|f| f.has_defaults()).unwrap_or(false) {
                existing_has_defaults = true;
                break;
            }
        }
        if info.has_defaults() || existing_has_defaults {
            return Err(SymbolError::new(format!(
                "function '{}' cannot be overloaded because it uses default parameter values",
                base
            )));
        }
        // Promote a lone bare singleton to its mangled key the moment a second overload appears.
        if existing.len() == 1 && existing[0] == base {
            if let Some(mut first) = self.functions.remove(base) {
                let first_key = overload_key(base, &first.parameters, type_ctx);
                first.name = first_key.clone();
                self.functions.insert(first_key.clone(), first);
                existing[0] = first_key;
            }
        }
        let key = overload_key(base, &info.parameters, type_ctx);
        if self.functions.contains_key(&key) {
            return Err(SymbolError::new(format!(
                "Duplicate overload: '{}' with the same parameter types is already defined",
                base
            )));
        }
        info.name = key.clone();
        existing.push(key.clone());
        self.functions.insert(key.clone(), info);
        Ok(key)
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
    pub fn resolve_emitted_name(&self, base: &str, parameters: &[String], type_ctx: &mut crate::types::TypeCtx) -> String {
        if self.is_overloaded(base) {
            overload_key(base, parameters, type_ctx)
        } else {
            base.to_string()
        }
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
            if info.parameters.len() != args.len() {
                continue;
            }
            let mut score = 0i32;
            let mut viable = true;
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
    pub intrinsic_name: Option<String>,
    /// True when the declaration is marked `public`. For methods this gates external calls
    /// (private methods may only be called from within their declaring type). Defaults to `true`
    /// for synthesized/stdlib entries so they are callable everywhere.
    pub is_public: bool,
}

impl FunctionTableInfo {
    pub fn new(
        name: String,
        return_type: Option<Type>,
        parameters: Vec<String>,
    ) -> FunctionTableInfo {
        let defaults = vec![None; parameters.len()];
        FunctionTableInfo {
            name,
            return_type,
            parameters,
            defaults,
            is_async: false,
            is_static: false,
            intrinsic_name: None,
            is_public: true,
        }
    }
    pub fn from(func: &FunctionNode) -> Self {
        let name = func.name.clone();
        let return_type = func.return_type.clone();
        let mut parameters: Vec<String> = vec![];
        let mut defaults: Vec<Option<Type>> = vec![];
        for i in func.parameters.iter() {
            let j = i.clone();
            parameters.push(j.type_.get_type());
            defaults.push(j.default);
        }
        let intrinsic_name = crate::intrinsics::intrinsic_key(&func.attributes);
        let mut info = FunctionTableInfo::new(name.text, return_type, parameters);
        info.defaults = defaults;
        info.is_async = func.is_async;
        info.is_static = func.is_static;
        info.intrinsic_name = intrinsic_name;
        // `extern` functions/methods are interop entry points (WASM imports): they cannot be
        // host-exported and privacy is meaningless for them, so they are always call-visible.
        info.is_public = func.is_public || func.is_extern;
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
