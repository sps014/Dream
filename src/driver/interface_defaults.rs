//! Interface default-method support: when an `interface` method supplies a default body, every
//! class that implements the interface but omits the method inherits that body. Rather than teach
//! the many method-iteration sites (registration, body analysis, itable emission) about defaults,
//! we synthesize an `extend <Class> { <default method> }` block per (class, missing default) and
//! append it to the program's `extends`. The cloned method's `this` binds to the concrete class,
//! so its calls to the interface's other methods resolve and dispatch exactly like a hand-written
//! method would. This mirrors the `@json` derive strategy (synthesize + reuse the normal path).

use crate::syntax::nodes::interface_node::InterfaceDeclarationNode;
use crate::syntax::nodes::struct_node::StructDeclarationNode;
use crate::syntax::nodes::{ExtendNode, FunctionNode};

/// Strips any generic argument suffix (`Container<int>` -> `Container`) and nullable marker so a
/// spelled interface type resolves to its declared name.
fn bare_name(spelled: &str) -> &str {
    let no_nullable = spelled.strip_suffix('?').unwrap_or(spelled);
    match no_nullable.find('<') {
        Some(i) => &no_nullable[..i],
        None => no_nullable,
    }
}

/// For each class that implements an interface with default methods it does not itself define,
/// appends a synthesized `extend <Class> { ... }` block carrying the inherited default bodies.
pub(crate) fn generate_interface_default_impls<'a>(
    all_structs: &[StructDeclarationNode<'a>],
    all_interfaces: &[InterfaceDeclarationNode<'a>],
    all_extends: &mut Vec<ExtendNode<'a>>,
) {
    // Index the (non-generic) interfaces that actually carry defaults; skip generic interfaces,
    // whose defaults would need per-instantiation substitution (deferred).
    let ifaces_with_defaults: Vec<&InterfaceDeclarationNode<'a>> = all_interfaces
        .iter()
        .filter(|i| {
            i.generic_parameters.is_none() && i.methods.iter().any(|m| m.is_default_impl)
        })
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
            let spelled = impl_ty.get_type();
            let iface_name = bare_name(&spelled);
            let Some(iface) = ifaces_with_defaults.iter().find(|i| i.name.text == iface_name)
            else {
                continue;
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
                inherited.push(m);
            }
        }

        if !inherited.is_empty() {
            synthesized.push(ExtendNode::new(class.name.clone(), None, inherited));
        }
    }

    all_extends.extend(synthesized);
}
