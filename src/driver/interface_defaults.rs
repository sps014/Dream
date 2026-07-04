//! Interface default-method support: when an `interface` method supplies a default body, every
//! class that implements the interface but omits the method inherits that body. Rather than teach
//! the many method-iteration sites (registration, body analysis, itable emission) about defaults,
//! we synthesize an `extend <Class> { <default method> }` block per (class, missing default) and
//! append it to the program's `extends`. The cloned method's `this` binds to the concrete class,
//! so its calls to the interface's other methods resolve and dispatch exactly like a hand-written
//! method would. This mirrors the `@json` derive strategy (synthesize + reuse the normal path).

use crate::semantics::analyzer::{generic_bindings, substitute_generic_type};
use crate::syntax::nodes::interface_node::InterfaceDeclarationNode;
use crate::syntax::nodes::struct_node::StructDeclarationNode;
use crate::syntax::nodes::{ExtendNode, FunctionNode, Type};

/// The declared base name of an implemented interface type (`Container<int>` -> `"Container"`),
/// read from the identifier token so it matches the interface's declared name. (Note `get_type()`
/// would yield the *mangled* `Container_int`, which never matches.)
fn interface_base_name(impl_ty: &Type) -> Option<&str> {
    match impl_ty {
        Type::Struct(token, _) => Some(token.text.as_str()),
        _ => None,
    }
}

/// The concrete generic arguments an `implements` clause supplies to an interface, e.g.
/// `Container<int>` yields `[int]`; a bare `Animal` yields `[]`.
fn implemented_args(impl_ty: &Type) -> &[Type] {
    match impl_ty {
        Type::Struct(_, Some(args)) => args,
        _ => &[],
    }
}

/// For each class that implements an interface with default methods it does not itself define,
/// appends a synthesized `extend <Class> { ... }` block carrying the inherited default bodies.
///
/// Generic interfaces are supported: the interface's type parameters are substituted with the
/// arguments spelled in the `implements` clause (`Container<int>` binds the interface's `T` to
/// `int`, `Container<T>` binds it to the class's own `T`) throughout the inherited method's
/// signature. The shared body operates on parameter names and `this`, so it is reused by reference;
/// a synthesized default for a generic class carries the class's generic parameters so it is
/// monomorphized alongside the class.
pub(crate) fn generate_interface_default_impls<'a>(
    all_structs: &[StructDeclarationNode<'a>],
    all_interfaces: &[InterfaceDeclarationNode<'a>],
    all_extends: &mut Vec<ExtendNode<'a>>,
) {
    // Index every interface (generic or not) that actually carries defaults.
    let ifaces_with_defaults: Vec<&InterfaceDeclarationNode<'a>> = all_interfaces
        .iter()
        .filter(|i| i.methods.iter().any(|m| m.is_default_impl))
        .collect();
    if ifaces_with_defaults.is_empty() {
        return;
    }

    let mut synthesized: Vec<ExtendNode<'a>> = Vec::new();
    for class in all_structs {
        if class.implements.is_empty() {
            continue;
        }
        // A default is only needed when the class defines neither the method itself nor an
        // `extend`-block method of the same name (which would already satisfy the interface).
        let defines = |name: &str| -> bool {
            class.methods.iter().any(|m| m.name.text == name)
                || all_extends.iter().any(|e| {
                    e.target.text == class.name.text
                        && e.methods.iter().any(|m| m.name.text == name)
                })
                || synthesized.iter().any(|e| {
                    e.target.text == class.name.text
                        && e.methods.iter().any(|m| m.name.text == name)
                })
        };

        let mut inherited: Vec<FunctionNode<'a>> = Vec::new();
        for impl_ty in &class.implements {
            let Some(iface_name) = interface_base_name(impl_ty) else {
                continue;
            };
            let Some(iface) = ifaces_with_defaults
                .iter()
                .find(|i| i.name.text == iface_name)
            else {
                continue;
            };
            // Map the interface's type parameters to the concrete arguments from the `implements`
            // clause. Empty (identity) for a non-generic interface.
            let bindings = match &iface.generic_parameters {
                Some(params) => generic_bindings(params, implemented_args(impl_ty)),
                None => Default::default(),
            };
            for method in iface.methods.iter() {
                if !method.is_default_impl || method.is_static {
                    continue;
                }
                if defines(&method.name.text) {
                    continue;
                }
                // Clone the interface's default into a concrete class method. The body slice is
                // arena-allocated and shared by reference; `this` rebinds to the class during
                // method registration. Clear the interface-only marker on the copy.
                let mut m = method.clone();
                m.is_default_impl = false;
                // Substitute the interface's type parameters in the signature so, e.g.,
                // `fun lessThan(other: T): bool` becomes `fun lessThan(other: int): bool` for a
                // `Container<int>` implementer (or stays `T`, the class's own parameter, for a
                // `Container<T>` implementer, which the class-level monomorphization then resolves).
                if !bindings.is_empty() {
                    if let Some(ret) = &m.return_type {
                        m.return_type = Some(substitute_generic_type(ret, &bindings));
                    }
                    for param in &mut m.parameters {
                        param.type_ = substitute_generic_type(&param.type_, &bindings);
                    }
                }
                inherited.push(m);
            }
        }

        if !inherited.is_empty() {
            // A generic class's extension must carry the class's parameters so the inherited default
            // is monomorphized with each instantiation; a non-generic class contributes `None`.
            synthesized.push(ExtendNode::new(
                class.name.clone(),
                class.generic_parameters.clone(),
                inherited,
            ));
        }
    }

    all_extends.extend(synthesized);
}
