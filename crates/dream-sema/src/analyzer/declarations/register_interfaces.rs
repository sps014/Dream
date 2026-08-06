//! Interface declarations, monomorphization, and implementation validation: registering interface
//! defs + method slots, instantiating generic interface templates, building the runtime interface
//! table, the interface-membership/assignability queries, and `validate_implements` (checking a
//! class satisfies each interface it names). These are `impl Analyzer` methods, kept in the
//! `declarations` module alongside the other top-level registration passes.

use super::*;
use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::types::mangle_generic;
use dream_syntax::nodes::{FunctionNode, ProgramNode, Type};
use dream_types::method_fn;
use std::collections::HashMap;

impl<'a> Analyzer<'a> {
    /// Pass: register every interface's `DefId` and its method signatures. Interfaces declare method
    /// signatures (no fields in v1); a method may carry a default body that implementers inherit (see
    /// `driver::interface_defaults`). Generic interfaces are stashed as templates and monomorphized on
    /// demand. The declaration order of methods is their local index (used later for itable slots).
    ///
    /// Interfaces may extend parents (`interface Child : Parent + Other`). Parent relationships are
    /// recorded here; method lists for non-generic interfaces are flattened in
    /// [`finalize_interface_inheritance`] after every interface name is known. Generic instances
    /// flatten inside [`ensure_interface_instantiated`].
    pub(in crate::analyzer) fn register_interfaces(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        for iface in node.interfaces.iter() {
            diagnostics.file_path = file_path_string(&iface.file_path);
            self.type_visibility.insert(
                iface.name.text.clone(),
                (iface.file_path.clone(), iface.visibility),
            );
            self.type_ctx.register(
                DefKind::Interface,
                &iface.name.text,
                generic_param_names(&iface.generic_parameters),
            );
            for method in iface.methods.iter() {
                if method.is_static {
                    diagnostics.report_error(
                        format!(
                            "Interface method '{}' cannot be 'static' (interface methods are dynamically dispatched instance methods)",
                            method.name.text
                        ),
                        Some(method.name.position),
                    );
                }
            }

            if self
                .interface_decls
                .insert(iface.name.text.clone(), iface)
                .is_some()
            {
                diagnostics.report_error(
                    format!("Interface '{}' is already defined", iface.name.text),
                    Some(iface.name.position),
                );
            }
            self.interface_parents
                .insert(iface.name.text.clone(), iface.parents.clone());

            if iface.generic_parameters.is_some() {
                if self
                    .generic_interfaces
                    .insert(iface.name.text.clone(), iface)
                    .is_some()
                {
                    diagnostics.report_error(
                        format!("Interface '{}' is already defined", iface.name.text),
                        Some(iface.name.position),
                    );
                }
                continue;
            }

            // Own methods only for now; [`finalize_interface_inheritance`] merges parents.
            let methods: Vec<&'a FunctionNode<'a>> =
                iface.methods.iter().filter(|m| !m.is_static).collect();
            if self
                .interface_methods
                .insert(iface.name.text.clone(), methods)
                .is_some()
            {
                diagnostics.report_error(
                    format!("Interface '{}' is already defined", iface.name.text),
                    Some(iface.name.position),
                );
            }
        }
        self.finalize_interface_inheritance(diagnostics);
    }

    /// After every interface name is registered, flatten non-generic interfaces that extend parents
    /// so their `interface_methods` entries include the inherited closure (and diagnose cycles /
    /// ambiguous defaults).
    fn finalize_interface_inheritance(&mut self, diagnostics: &mut DiagnosticBag) {
        let names: Vec<String> = self
            .interface_decls
            .iter()
            .filter(|(_, d)| d.generic_parameters.is_none())
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            let _ = self.flatten_interface_methods(&name, &[], diagnostics, &mut Vec::new());
        }
    }

    /// Instantiates a generic interface `base<args>` into a concrete `interface_methods` entry
    /// (e.g. `Container<int>` -> `Container_int`) by substituting the type parameters through every
    /// method signature (including inherited parents). Mirrors [`ensure_struct_instantiated`];
    /// idempotent.
    pub(in crate::analyzer) fn ensure_interface_instantiated(
        &mut self,
        base_name: &str,
        args: &[Type],
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        if args.is_empty() {
            // Non-generic interfaces are flattened in `finalize_interface_inheritance`.
            return;
        }
        let mangled = mangle_generic(base_name, args);
        self.type_ctx
            .register_instance(DefKind::Interface, base_name, args);
        self.type_ctx
            .register(DefKind::Interface, &mangled, Vec::new());
        if !self.interface_methods.contains_key(&mangled) {
            let template = match self.interface_decls.get(base_name) {
                Some(t) => *t,
                None => return,
            };
            let params = template.generic_parameters.as_deref().unwrap_or(&[]);
            Self::check_generic_arity(
                "interface",
                base_name,
                params.len(),
                args.len(),
                position,
                diagnostics,
            );
            self.flatten_interface_methods(base_name, args, diagnostics, &mut Vec::new());
        }
        // Parent flattening (IndexedCollection → Collection) may have created `Collection_int`
        // already; still attach package `extend Collection<T>` methods onto that name.
        if self.interface_extensions_attached.insert(mangled.clone()) {
            self.register_generic_extension_methods(base_name, &mangled, args, diagnostics);
        }
    }

    /// Builds (or rebuilds) the flattened method list for interface `base_name` with concrete
    /// `args` (empty for non-generic). Returns the mangled/concrete interface name.
    /// `stack` tracks the base names currently being flattened to diagnose inheritance cycles.
    fn flatten_interface_methods(
        &mut self,
        base_name: &str,
        args: &[Type],
        diagnostics: &mut DiagnosticBag,
        stack: &mut Vec<String>,
    ) -> Option<String> {
        let key = if args.is_empty() {
            base_name.to_string()
        } else {
            mangle_generic(base_name, args)
        };

        if stack.iter().any(|s| s == base_name) {
            if let Some(decl) = self.interface_decls.get(base_name) {
                diagnostics.file_path = file_path_string(&decl.file_path);
            }
            diagnostics.report_error(
                format!(
                    "interface inheritance cycle involving '{}'",
                    stack.join(" -> ") + " -> " + base_name
                ),
                self.interface_decls
                    .get(base_name)
                    .map(|d| d.name.position),
            );
            return None;
        }

        // Generic instance already flattened.
        if !args.is_empty() && self.interface_methods.contains_key(&key) {
            return Some(key);
        }

        let template = match self.interface_decls.get(base_name) {
            Some(t) => *t,
            None => return None,
        };
        let params = template.generic_parameters.as_deref().unwrap_or(&[]);
        let bindings = if params.is_empty() {
            Default::default()
        } else {
            generic_bindings(params, args)
        };

        stack.push(base_name.to_string());

        let mut merged: Vec<&'a FunctionNode<'a>> = Vec::new();
        let mut from_parent: HashMap<String, (bool, String)> = HashMap::new();
        let mut parent_keys: Vec<String> = Vec::new();

        let parents = template.parents.clone();
        for parent_ty in &parents {
            let Some((pbase, pargs_raw)) = Self::resolve_struct_parts(parent_ty) else {
                diagnostics.report_error(
                    format!(
                        "interface '{}' parent must be an interface type, got {}",
                        base_name,
                        parent_ty.get_type()
                    ),
                    parent_ty.get_span().or(Some(template.name.position)),
                );
                continue;
            };
            if !self.interface_decls.contains_key(&pbase) {
                diagnostics.report_error(
                    format!(
                        "interface '{}' cannot extend '{}': not an interface",
                        base_name, pbase
                    ),
                    parent_ty.get_span().or(Some(template.name.position)),
                );
                continue;
            }
            let pargs: Vec<Type> = pargs_raw
                .iter()
                .map(|t| substitute_generic_type(t, &bindings))
                .collect();
            if !pargs.is_empty() {
                self.type_ctx
                    .register_instance(DefKind::Interface, &pbase, &pargs);
                self.type_ctx.register(
                    DefKind::Interface,
                    &mangle_generic(&pbase, &pargs),
                    Vec::new(),
                );
            }
            let Some(parent_key) =
                self.flatten_interface_methods(&pbase, &pargs, diagnostics, stack)
            else {
                continue;
            };
            if !parent_keys.contains(&parent_key) {
                parent_keys.push(parent_key.clone());
            }
            let parent_methods = self
                .interface_methods
                .get(&parent_key)
                .cloned()
                .unwrap_or_default();
            for pm in parent_methods {
                let name = pm.name.text.clone();
                if let Some((prev_default, prev_src)) = from_parent.get(&name) {
                    if *prev_default && pm.is_default_impl {
                        diagnostics.report_error(
                            format!(
                                "interface '{}': ambiguous default for method '{}' inherited from both '{}' and '{}'; override it on '{}'",
                                base_name, name, prev_src, parent_key, base_name
                            ),
                            Some(template.name.position),
                        );
                    }
                    continue;
                }
                from_parent.insert(name, (pm.is_default_impl, parent_key.clone()));
                merged.push(pm);
            }
        }

        let own: Vec<&'a FunctionNode<'a>> = if args.is_empty() {
            template.methods.iter().filter(|m| !m.is_static).collect()
        } else {
            let mut owned = Vec::new();
            for method in template.methods.iter().filter(|m| !m.is_static) {
                let mut m = method.clone();
                Self::substitute_generic_signature(&mut m, &bindings);
                let method_ref: &'a FunctionNode<'a> = self.arena.alloc(m);
                owned.push(method_ref);
            }
            owned
        };

        for om in own {
            if let Some(pos) = merged.iter().position(|m| m.name.text == om.name.text) {
                merged[pos] = om;
            } else {
                merged.push(om);
            }
        }

        stack.pop();
        self.interface_parent_instances
            .insert(key.clone(), parent_keys);
        self.interface_methods.insert(key.clone(), merged);
        Some(key)
    }

    /// Appends `iface_name` and all interfaces it extends (transitively) into `out`.
    pub(in crate::analyzer) fn collect_interface_ancestors(
        &self,
        iface_name: &str,
        out: &mut Vec<String>,
    ) {
        if out.iter().any(|n| n == iface_name) {
            return;
        }
        out.push(iface_name.to_string());
        if let Some(parents) = self.interface_parent_instances.get(iface_name) {
            for p in parents.clone() {
                self.collect_interface_ancestors(&p, out);
            }
        }
    }

    /// Builds the interface dispatch metadata carried into codegen: the ordered interfaces (index =
    /// `iface_id`) with each method slot's `call_indirect` signature, and, per implementing class,
    /// the concrete method symbol filling each `(interface, slot)`.
    pub(in crate::analyzer) fn hir_build_interfaces(
        &mut self,
    ) -> dream_hir::InterfaceTable {
        use dream_hir::{InterfaceImpl, InterfaceInfo, InterfaceTable};

        let iface_order: Vec<(String, Vec<&'a FunctionNode<'a>>)> = self
            .interface_methods
            .iter()
            .map(|(name, methods)| (name.clone(), methods.clone()))
            .collect();

        let mut name_to_id: HashMap<String, usize> = HashMap::new();
        let mut interfaces = Vec::with_capacity(iface_order.len());
        for (id, (name, methods)) in iface_order.iter().enumerate() {
            name_to_id.insert(name.clone(), id);
            let sigs: Vec<dream_types::TypeId> = methods
                .iter()
                .map(|m| self.interface_dispatch_sig(m))
                .collect();
            interfaces.push(InterfaceInfo {
                name: name.clone(),
                method_count: methods.len(),
                sigs,
            });
        }

        let class_impls: Vec<(String, Vec<String>)> = self
            .implements
            .iter()
            .map(|(class, ifaces)| (class.clone(), ifaces.clone()))
            .collect();
        let mut impls = Vec::new();
        for (class, ifaces) in class_impls {
            let class_ty = self.type_ctx.lower_str(&class);
            let mut entries = Vec::new();
            for iface in ifaces {
                let Some(&id) = name_to_id.get(&iface) else {
                    continue;
                };
                let methods = self
                    .interface_methods
                    .get(&iface)
                    .cloned()
                    .unwrap_or_default();
                let symbols: Vec<String> = methods
                    .iter()
                    .map(|m| method_fn(&class, &m.name.text))
                    .collect();
                entries.push((id, symbols));
            }
            impls.push(InterfaceImpl { class_ty, entries });
        }

        InterfaceTable { interfaces, impls }
    }

    /// True when `name` (a bare type name, no array suffix) is a registered interface.
    /// Recognizes both plain interfaces (`Animal`) and mangled generic interface instances
    /// (`Container_int`), even before the latter has been instantiated.
    pub(in crate::analyzer) fn is_interface_name(&self, name: &str) -> bool {
        self.type_ctx.nominal_kind(name) == Some(DefKind::Interface)
            || self.demangle_generic_interface(name).is_some()
    }

    /// True when `name` is the base name of a declared generic interface (`Container`).
    pub(in crate::analyzer) fn is_generic_interface(&self, name: &str) -> bool {
        self.generic_interfaces.contains_key(name)
    }

    /// Splits a mangled generic interface name (e.g. `Container_int`) into its base name and
    /// concrete type argument, choosing the split so the base is a registered generic interface.
    /// Mirrors [`demangle_generic_struct`].
    pub(in crate::analyzer) fn demangle_generic_interface(
        &self,
        mangled: &str,
    ) -> Option<(String, String)> {
        let parts: Vec<&str> = mangled.split('_').collect();
        for split in 1..parts.len() {
            let base = parts[..split].join("_");
            if self.generic_interfaces.contains_key(&base) {
                return Some((base, parts[split..].join("_")));
            }
        }
        None
    }

    /// True when class `class_name` was validated as implementing interface `iface_name`.
    pub(in crate::analyzer) fn class_implements(
        &self,
        class_name: &str,
        iface_name: &str,
    ) -> bool {
        self.implements
            .get(class_name)
            .is_some_and(|ifaces| ifaces.iter().any(|i| i == iface_name))
    }

    /// True when a value of type `value` may be implicitly converted to interface-typed `target`
    /// (an upcast): `target` names an interface and `value`'s concrete class implements it.
    pub(in crate::analyzer) fn value_assignable_to_interface(
        &self,
        target: &Type,
        value: &Type,
    ) -> bool {
        let iface = target.get_type();
        if !self.is_interface_name(&iface) {
            return false;
        }
        let val = value.get_type();
        self.implements_as_interface_ref(&val, &iface)
    }

    /// True when `class_name` may be implicitly/explicitly widened to an interface *reference*
    /// (`iface_name`). A reference class upcasts by identity (same tagged pointer); a value
    /// (`struct`) type is *boxed* into a fresh tagged heap object at the upcast site (see the value
    /// struct case in `emit_cast`), so it too may become an interface reference.
    pub(in crate::analyzer) fn implements_as_interface_ref(
        &self,
        class_name: &str,
        iface_name: &str,
    ) -> bool {
        self.class_implements(class_name, iface_name)
    }

    /// True when `iface_method` and `class_method` have matching signatures (same parameter types
    /// in order, matching return types, and the same async-ness). Return types may be an exact
    /// match or a class return that is assignable to the interface return (e.g. a concrete
    /// `ListIterator<T>` for an `Iterator<T>` interface requirement). An `async` interface method
    /// must be implemented by an `async` method (and vice versa) because the two dispatch to
    /// different code shapes (a `Future`-producing constructor vs. a plain call).
    fn interface_method_matches(
        &mut self,
        iface_method: &FunctionNode,
        class_method: &FunctionNode,
        bindings: &GenericBindings,
        _diagnostics: &mut DiagnosticBag,
    ) -> bool {
        if iface_method.is_async != class_method.is_async {
            return false;
        }
        if iface_method.parameters.len() != class_method.parameters.len() {
            return false;
        }
        for (a, b) in iface_method
            .parameters
            .iter()
            .zip(class_method.parameters.iter())
        {
            let a_ty = substitute_generic_type(&a.type_, bindings);
            let b_ty = substitute_generic_type(&b.type_, bindings);
            if a_ty.get_type() != b_ty.get_type() {
                return false;
            }
        }
        let iface_ret = iface_method
            .return_type
            .as_ref()
            .map(|t| substitute_generic_type(t, bindings))
            .unwrap_or(Type::Void);
        let class_ret = class_method
            .return_type
            .as_ref()
            .map(|t| substitute_generic_type(t, bindings))
            .unwrap_or(Type::Void);
        // Exact match, or the class return type is assignable to the interface return
        // (e.g. `ListIterator<T>` implementing `Iterator<T>`).
        if iface_ret.get_type() == class_ret.get_type() {
            return true;
        }
        self.type_str_assignable(&iface_ret.get_type(), &class_ret.get_type())
    }

    /// Validates a class's `implements` clause: every listed type must name an interface, and the
    /// class must provide an instance method with a matching signature for each interface method.
    /// Records the validated (mangled) interface list in `self.implements` under `class_name`.
    ///
    /// Works uniformly for non-generic classes (`bindings` empty) and monomorphized generic classes
    /// (`bindings` maps the class's type parameters to concrete types). For a monomorphized class,
    /// the `implements` entries are expected to already be substituted (e.g. `Container<int>`) while
    /// `methods` are the unsubstituted template methods, substituted here for signature comparison.
    /// Generic interfaces named in the clause are instantiated on demand.
    pub(in crate::analyzer) fn validate_implements(
        &mut self,
        class_name: &str,
        implements: &[Type],
        methods: &[FunctionNode<'a>],
        bindings: &GenericBindings,
        class_pos: TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        if implements.is_empty() {
            return;
        }
        let mut validated: Vec<String> = Vec::new();
        for iface_ty in implements {
            let span = iface_ty.get_span().unwrap_or(class_pos);
            let (base, args) = match Self::resolve_struct_parts(iface_ty) {
                Some(parts) => parts,
                None => continue,
            };
            if !self.is_interface_name(&base) {
                diagnostics.report_error(
                    format!(
                        "'{}' is not an interface (class '{}' can only implement interfaces)",
                        base, class_name
                    ),
                    Some(span),
                );
                continue;
            }
            let iface_name = if args.is_empty() {
                base.clone()
            } else {
                self.ensure_interface_instantiated(&base, &args, &span, diagnostics);
                mangle_generic(&base, &args)
            };
            let iface_methods = match self.interface_methods.get(&iface_name) {
                Some(m) => m.clone(),
                None => continue,
            };
            for im in &iface_methods {
                match methods
                    .iter()
                    .find(|cm| cm.name.text == im.name.text && !cm.is_static)
                {
                    Some(cm) => {
                        let matches = if bindings.is_empty() {
                            self.interface_method_matches(im, cm, bindings, diagnostics)
                        } else {
                            let mut sub = cm.clone();
                            Self::substitute_generic_signature(&mut sub, bindings);
                            self.interface_method_matches(im, &sub, bindings, diagnostics)
                        };
                        if !matches {
                            diagnostics.report_error(
                                format!(
                                    "class '{}' method '{}' does not match the signature required by interface '{}'",
                                    class_name, im.name.text, iface_name
                                ),
                                Some(cm.name.position),
                            );
                        }
                    }
                    None if im.is_default_impl => {
                        // Satisfied by the interface's default body, which is injected as an
                        // `extend <class> { ... }` method before analysis (see
                        // `generate_interface_default_impls`), so the class need not declare it.
                    }
                    None => {
                        diagnostics.report_error(
                            format!(
                                "class '{}' does not implement method '{}' required by interface '{}'",
                                class_name, im.name.text, iface_name
                            ),
                            Some(class_pos),
                        );
                    }
                }
            }
            if !validated.contains(&iface_name) {
                // Explicit implement plus every parent interface (subtype relationship).
                let mut ancestors = Vec::new();
                self.collect_interface_ancestors(&iface_name, &mut ancestors);
                for a in ancestors {
                    if !validated.contains(&a) {
                        validated.push(a);
                    }
                }
            }
            // Attach `extend Collection<T>`-style package methods onto this class so
            // `list.to_list()` resolves without going through the interface receiver.
            self.attach_interface_extension_methods(&base, &args, class_name, diagnostics);
        }
        // Merge into any interfaces already recorded for this type (a class may gain further
        // interfaces through an `extend : Iface` block) rather than replacing them.
        let entry = self.implements.entry(class_name.to_string()).or_default();
        for iface in validated {
            if !entry.contains(&iface) {
                entry.push(iface);
            }
        }
    }

    /// Registers package `extend Iface<…>` methods onto `target` (a concrete class or interface
    /// instance name), walking parent interfaces so `extend Collection<T>` applies when the class
    /// only declares `IndexedCollection<T>`.
    fn attach_interface_extension_methods(
        &mut self,
        base_name: &str,
        args: &[Type],
        target: &str,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mut stack = vec![(base_name.to_string(), args.to_vec())];
        let mut seen = Vec::new();
        while let Some((base, args)) = stack.pop() {
            if seen.iter().any(|s| s == &base) {
                continue;
            }
            seen.push(base.clone());
            self.register_generic_extension_methods(&base, target, &args, diagnostics);
            let parents = self
                .interface_parents
                .get(&base)
                .cloned()
                .unwrap_or_default();
            let params = self
                .interface_decls
                .get(&base)
                .and_then(|d| d.generic_parameters.clone())
                .unwrap_or_default();
            let bindings = if params.is_empty() {
                Default::default()
            } else {
                generic_bindings(&params, &args)
            };
            for parent_ty in &parents {
                let Some((pbase, pargs_raw)) = Self::resolve_struct_parts(parent_ty) else {
                    continue;
                };
                let pargs: Vec<Type> = pargs_raw
                    .iter()
                    .map(|t| substitute_generic_type(t, &bindings))
                    .collect();
                stack.push((pbase, pargs));
            }
        }
    }
}
