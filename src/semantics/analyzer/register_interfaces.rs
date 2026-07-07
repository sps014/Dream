//! Interface declarations, monomorphization, and implementation validation. Split out of
//! `declarations.rs`: registering interface defs + method slots, instantiating generic interface
//! templates, building the runtime interface table, the interface-membership/assignability
//! queries, and `validate_implements` (checking a class satisfies each interface it names). These
//! are methods on `Analyzer`, defined in a sibling module so they share its `pub(super)` surface.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::syntax::nodes::types::{mangle_generic, strip_nullable};
use crate::syntax::nodes::{FunctionNode, ProgramNode, Type};
use crate::types::method_fn;

impl<'a> Analyzer<'a> {
    /// Pass: register every interface's `DefId` and its method signatures. Interfaces declare method
    /// signatures (no fields in v1); a method may carry a default body that implementers inherit (see
    /// `driver::interface_defaults`). Generic interfaces are stashed as templates and monomorphized on
    /// demand. The declaration order of methods is their local index (used later for itable slots).
    pub(super) fn register_interfaces(
        &mut self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        for iface in node.interfaces.iter() {
            diagnostics.file_path = file_path_string(&iface.file_path);
            self.type_visibility.insert(
                iface.name.text.clone(),
                (iface.file_path.clone(), iface.is_public),
            );
            self.type_ctx.register(
                DefKind::Interface,
                &iface.name.text,
                generic_param_names(&iface.generic_parameters),
            );
            // `static` methods are rejected on any interface (interface methods are dynamically
            // dispatched instance methods). `async` interface methods are supported: they dispatch
            // to a concrete async implementation that returns a `Future<T>`.
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

            // Generic interfaces are stashed as templates and monomorphized on demand (see
            // `ensure_interface_instantiated`); only concrete instances get itable method slots.
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
    }

    /// Instantiates a generic interface `base<args>` into a concrete `interface_methods` entry
    /// (e.g. `Container<int>` -> `Container_int`) by substituting the type parameters through every
    /// method signature. Mirrors [`ensure_struct_instantiated`]; idempotent. The concrete instance
    /// becomes an ordinary interface with its own itable slots at `hir_build_interfaces` time.
    pub(super) fn ensure_interface_instantiated(
        &mut self,
        base_name: &str,
        args: &[Type],
        position: &TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mangled = mangle_generic(base_name, args);
        // Canonicalize the mangled name to the structured `(base def, args)` interface id, and
        // register the mangled name as a nominal interface so `is_interface_name` recognizes it.
        self.type_ctx
            .register_instance(DefKind::Interface, base_name, args);
        self.type_ctx
            .register(DefKind::Interface, &mangled, Vec::new());
        if self.interface_methods.contains_key(&mangled) {
            return;
        }
        let template = match self.generic_interfaces.get(base_name) {
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
        let bindings = generic_bindings(params, args);
        let mut methods: Vec<&'a FunctionNode<'a>> = Vec::new();
        for method in template.methods.iter().filter(|m| !m.is_static) {
            let mut m = method.clone();
            Self::substitute_generic_signature(&mut m, &bindings);
            methods.push(self.arena.alloc(m));
        }
        self.interface_methods.insert(mangled, methods);
    }

    /// Builds the interface dispatch metadata carried into codegen: the ordered interfaces (index =
    /// `iface_id`) with each method slot's `call_indirect` signature, and, per implementing class,
    /// the concrete method symbol filling each `(interface, slot)`.
    pub(super) fn hir_build_interfaces(&mut self) -> crate::hir::InterfaceTable {
        use crate::hir::{InterfaceImpl, InterfaceInfo, InterfaceTable};

        let iface_order: Vec<(String, Vec<&'a FunctionNode<'a>>)> = self
            .interface_methods
            .iter()
            .map(|(name, methods)| (name.clone(), methods.clone()))
            .collect();

        let mut name_to_id: HashMap<String, usize> = HashMap::new();
        let mut interfaces = Vec::with_capacity(iface_order.len());
        for (id, (name, methods)) in iface_order.iter().enumerate() {
            name_to_id.insert(name.clone(), id);
            let sigs: Vec<crate::types::TypeId> = methods
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

    /// True when `name` (a bare type name, no nullable/array suffix) is a registered interface.
    /// Recognizes both plain interfaces (`Animal`) and mangled generic interface instances
    /// (`Container_int`), even before the latter has been instantiated.
    pub(super) fn is_interface_name(&self, name: &str) -> bool {
        self.type_ctx.nominal_kind(name) == Some(DefKind::Interface)
            || self.demangle_generic_interface(name).is_some()
    }

    /// True when `name` is the base name of a declared generic interface (`Container`).
    pub(super) fn is_generic_interface(&self, name: &str) -> bool {
        self.generic_interfaces.contains_key(name)
    }

    /// Splits a mangled generic interface name (e.g. `Container_int`) into its base name and
    /// concrete type argument, choosing the split so the base is a registered generic interface.
    /// Mirrors [`demangle_generic_struct`].
    fn demangle_generic_interface(&self, mangled: &str) -> Option<(String, String)> {
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
    pub(super) fn class_implements(&self, class_name: &str, iface_name: &str) -> bool {
        self.implements
            .get(class_name)
            .is_some_and(|ifaces| ifaces.iter().any(|i| i == iface_name))
    }

    /// True when a value of type `value` may be implicitly converted to interface-typed `target`
    /// (an upcast): `target` names an interface and `value`'s concrete class implements it.
    /// Nullable wrappers on either side are ignored.
    pub(super) fn value_assignable_to_interface(&self, target: &Type, value: &Type) -> bool {
        let iface = strip_nullable(&target.get_type()).to_string();
        if !self.is_interface_name(&iface) {
            return false;
        }
        let val = strip_nullable(&value.get_type()).to_string();
        self.implements_as_interface_ref(&val, &iface)
    }

    /// True when `class_name` may be implicitly/explicitly widened to an interface *reference*
    /// (`iface_name`). A reference class upcasts by identity (same tagged pointer); a value
    /// (`struct`) type is *boxed* into a fresh tagged heap object at the upcast site (see the value
    /// struct case in `emit_cast`), so it too may become an interface reference.
    pub(super) fn implements_as_interface_ref(&self, class_name: &str, iface_name: &str) -> bool {
        self.class_implements(class_name, iface_name)
    }

    /// True when `iface_method` and `class_method` have matching signatures (same parameter types
    /// in order, the same return type, and the same async-ness). Both are compared using their
    /// surface type spellings. An `async` interface method must be implemented by an `async` method
    /// (and vice versa) because the two dispatch to different code shapes (a `Future`-producing
    /// constructor vs. a plain call).
    fn interface_method_matches(iface_method: &FunctionNode, class_method: &FunctionNode) -> bool {
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
            if a.type_.get_type() != b.type_.get_type() {
                return false;
            }
        }
        let ret = |m: &FunctionNode| {
            m.return_type
                .as_ref()
                .map(|t| t.get_type())
                .unwrap_or_else(|| "void".to_string())
        };
        ret(iface_method) == ret(class_method)
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
    pub(super) fn validate_implements(
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
                            Self::interface_method_matches(im, cm)
                        } else {
                            let mut sub = cm.clone();
                            Self::substitute_generic_signature(&mut sub, bindings);
                            Self::interface_method_matches(im, &sub)
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
                validated.push(iface_name);
            }
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
}
