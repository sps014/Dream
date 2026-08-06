//! Interface default-method support: when an `interface` method supplies a default body, every
//! class that implements the interface but omits the method inherits that body. Rather than teach
//! the many method-iteration sites (registration, body analysis, itable emission) about defaults,
//! we synthesize an `extend <Class> { <default method> }` block per (class, missing default) and
//! append it to the program's `extends`. The cloned method's `this` binds to the concrete class,
//! so its calls to the interface's other methods resolve and dispatch exactly like a hand-written
//! method would. This mirrors the `@json` derive strategy (synthesize + reuse the normal path).
//!
//! Parent interfaces (`interface Child : Parent`) contribute their defaults too: implementing the
//! child inherits defaults from the full ancestor closure unless the child overrides the method.
//!
//! The same inheritance applies to `extend Target : Iface` blocks (e.g. `extend int[] :
//! IndexedCollection<int>`), so array/primitive implementers pick up `is_empty` / `all` / …

use dream_sema::analyzer::{generic_bindings, substitute_generic_type};
use dream_syntax::nodes::interface_node::InterfaceDeclarationNode;
use dream_syntax::nodes::struct_node::StructDeclarationNode;
use dream_syntax::nodes::{ExtendNode, FunctionNode, Type};
use dream_syntax::token::syntax_token::SyntaxToken;

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

/// Collect `(interface index in all_interfaces, concrete type args)` for `iface` and its parents
/// (child first so its defaults win over ancestors).
fn collect_iface_closure(
    iface_idx: usize,
    args: &[Type],
    all_interfaces: &[InterfaceDeclarationNode<'_>],
    visited: &mut Vec<String>,
    out: &mut Vec<(usize, Vec<Type>)>,
) {
    let iface = &all_interfaces[iface_idx];
    if visited.iter().any(|n| n == &iface.name.text) {
        return;
    }
    visited.push(iface.name.text.clone());
    out.push((iface_idx, args.to_vec()));

    let bindings = match &iface.generic_parameters {
        Some(params) => generic_bindings(params, args),
        None => Default::default(),
    };

    for parent_ty in &iface.parents {
        let Some(pbase) = interface_base_name(parent_ty) else {
            continue;
        };
        let Some(pidx) = all_interfaces.iter().position(|i| i.name.text == pbase) else {
            continue;
        };
        let pargs_raw = implemented_args(parent_ty);
        let pargs: Vec<Type> = pargs_raw
            .iter()
            .map(|t| substitute_generic_type(t, &bindings))
            .collect();
        collect_iface_closure(pidx, &pargs, all_interfaces, visited, out);
    }
}

fn collect_inherited_defaults<'a>(
    implements: &[Type],
    all_interfaces: &[InterfaceDeclarationNode<'a>],
    mut defines: impl FnMut(&str) -> bool,
) -> Vec<FunctionNode<'a>> {
    let mut inherited: Vec<FunctionNode<'a>> = Vec::new();
    let mut seen_methods: Vec<String> = Vec::new();
    for impl_ty in implements {
        let Some(iface_name) = interface_base_name(impl_ty) else {
            continue;
        };
        let Some(iface_idx) = all_interfaces.iter().position(|i| i.name.text == iface_name) else {
            continue;
        };
        let mut visited = Vec::new();
        let mut closure = Vec::new();
        collect_iface_closure(
            iface_idx,
            implemented_args(impl_ty),
            all_interfaces,
            &mut visited,
            &mut closure,
        );
        for (idx, args) in closure {
            let iface = &all_interfaces[idx];
            let bindings = match &iface.generic_parameters {
                Some(params) => generic_bindings(params, &args),
                None => Default::default(),
            };
            for method in iface.methods.iter() {
                if !method.is_default_impl || method.is_static {
                    continue;
                }
                if defines(&method.name.text) || seen_methods.iter().any(|n| n == &method.name.text)
                {
                    continue;
                }
                seen_methods.push(method.name.text.clone());
                let mut m = method.clone();
                m.is_default_impl = false;
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
    }
    inherited
}

fn push_default_extend<'a>(
    target: SyntaxToken,
    generic_parameters: Option<Vec<SyntaxToken>>,
    inherited: Vec<FunctionNode<'a>>,
    out: &mut Vec<ExtendNode<'a>>,
) {
    if inherited.is_empty() {
        return;
    }
    let mut ext = ExtendNode::new(target, generic_parameters, inherited);
    ext.is_synthesized = true;
    out.push(ext);
}

/// For each class (and each `extend Target : Iface` block) that implements an interface with
/// default methods it does not itself define, appends a synthesized `extend` block carrying the
/// inherited default bodies.
///
/// Generic interfaces are supported: the interface's type parameters are substituted with the
/// arguments spelled in the `implements` clause (`Container<int>` binds the interface's `T` to
/// `int`, `Container<T>` binds it to the class's own `T`) throughout the inherited method's
/// signature. Parent-interface defaults are included. The shared body operates on parameter names
/// and `this`, so it is reused by reference; a synthesized default for a generic class carries the
/// class's generic parameters so it is monomorphized alongside the class.
pub(crate) fn generate_interface_default_impls<'a>(
    all_structs: &[StructDeclarationNode<'a>],
    all_interfaces: &[InterfaceDeclarationNode<'a>],
    all_extends: &mut Vec<ExtendNode<'a>>,
) {
    let mut synthesized: Vec<ExtendNode<'a>> = Vec::new();
    for class in all_structs {
        if class.implements.is_empty() {
            continue;
        }
        let class_name = class.name.text.as_str();
        let inherited = collect_inherited_defaults(&class.implements, all_interfaces, |name| {
            class.methods.iter().any(|m| m.name.text == name)
                || all_extends.iter().any(|e| {
                    e.target.text == class_name && e.methods.iter().any(|m| m.name.text == name)
                })
                || synthesized.iter().any(|e| {
                    e.target.text == class_name && e.methods.iter().any(|m| m.name.text == name)
                })
        });
        push_default_extend(
            class.name.clone(),
            class.generic_parameters.clone(),
            inherited,
            &mut synthesized,
        );
    }

    // Snapshot extends that declare `implements` (e.g. concrete `extend int : Comparable<int>`).
    // Generic templates (`extend T[]`, `extend Collection<T>`) are skipped — they instantiate lazily.
    let extend_impl_indices: Vec<usize> = all_extends
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.implements.is_empty() && e.generic_parameters.is_none())
        .map(|(i, _)| i)
        .collect();
    for idx in extend_impl_indices {
        let target_name = all_extends[idx].target.text.clone();
        let implements = all_extends[idx].implements.clone();
        let own_methods: Vec<String> = all_extends[idx]
            .methods
            .iter()
            .map(|m| m.name.text.clone())
            .collect();
        let inherited = collect_inherited_defaults(&implements, all_interfaces, |name| {
            own_methods.iter().any(|m| m == name)
                || all_extends.iter().any(|e| {
                    e.target.text == target_name && e.methods.iter().any(|m| m.name.text == name)
                })
                || synthesized.iter().any(|e| {
                    e.target.text == target_name && e.methods.iter().any(|m| m.name.text == name)
                })
        });
        push_default_extend(
            all_extends[idx].target.clone(),
            all_extends[idx].generic_parameters.clone(),
            inherited,
            &mut synthesized,
        );
    }

    all_extends.extend(synthesized);
}
