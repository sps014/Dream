//! `@get_indexer` / `@set_indexer` / `@iterator` / `@next` registration: recognizes the
//! indexer/enumerator protocol attributes on a struct/class/`extend` method (their generic shape —
//! no args, method-only placement — is already validated by [`dream_abi::attributes`]), checks
//! role-specific arity/return rules, and records the mangled + surface method names so `obj[i]`,
//! `obj[i] = v`, and `for..in` desugaring can dispatch without bare-name lookup. One
//! [`ProtocolHooks`] table per registered type, keyed the same way as `register_methods_for`'s
//! `target_type_str`.

use super::*;
use dream_diagnostics::DiagnosticBag;
use dream_syntax::nodes::FunctionNode;
use dream_syntax::nodes::Type;

/// Which protocol role a method claims via its attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolRole {
    Get,
    Set,
    Iterator,
    Next,
}

impl ProtocolRole {
    fn from_attr_name(name: &str) -> Option<Self> {
        match name {
            "get_indexer" => Some(ProtocolRole::Get),
            "set_indexer" => Some(ProtocolRole::Set),
            "iterator" => Some(ProtocolRole::Iterator),
            "next" => Some(ProtocolRole::Next),
            _ => None,
        }
    }

    fn attr_name(self) -> &'static str {
        match self {
            ProtocolRole::Get => "get_indexer",
            ProtocolRole::Set => "set_indexer",
            ProtocolRole::Iterator => "iterator",
            ProtocolRole::Next => "next",
        }
    }

    /// Declared parameter count excluding the implicit `this`.
    fn expected_arity(self) -> usize {
        match self {
            ProtocolRole::Get => 1,
            ProtocolRole::Set => 2,
            ProtocolRole::Iterator | ProtocolRole::Next => 0,
        }
    }
}

/// A registered protocol-hook method: mangled name for HIR emission and surface name for
/// `MethodCall` desugar that re-enters ordinary method resolution.
#[derive(Debug, Clone)]
pub struct ProtocolHook {
    pub mangled_name: String,
    pub surface_name: String,
}

/// Per-type protocol hooks. At most one method per role.
#[derive(Debug, Clone, Default)]
pub struct ProtocolHooks {
    pub get: Option<ProtocolHook>,
    pub set: Option<ProtocolHook>,
    pub iterator: Option<ProtocolHook>,
    pub next: Option<ProtocolHook>,
}

impl ProtocolHooks {
    fn slot_mut(&mut self, role: ProtocolRole) -> &mut Option<ProtocolHook> {
        match role {
            ProtocolRole::Get => &mut self.get,
            ProtocolRole::Set => &mut self.set,
            ProtocolRole::Iterator => &mut self.iterator,
            ProtocolRole::Next => &mut self.next,
        }
    }

    fn slot(&self, role: ProtocolRole) -> &Option<ProtocolHook> {
        match role {
            ProtocolRole::Get => &self.get,
            ProtocolRole::Set => &self.set,
            ProtocolRole::Iterator => &self.iterator,
            ProtocolRole::Next => &self.next,
        }
    }
}

impl<'a> Analyzer<'a> {
    /// Recognizes `@get_indexer`/`@set_indexer`/`@iterator`/`@next` on `method` and records it
    /// against `target_type_str`. Reports role-specific shape rules the generic attribute layer
    /// cannot know, and rejects a second method claiming the same role on the same type.
    pub(in crate::analyzer) fn validate_and_register_protocol_hook(
        &mut self,
        target_type_str: &str,
        method: &FunctionNode<'a>,
        mangled_name: &str,
        diagnostics: &mut DiagnosticBag,
    ) {
        let role = method
            .attributes
            .iter()
            .find_map(|a| ProtocolRole::from_attr_name(a.name.text.as_str()));
        let Some(role) = role else {
            return;
        };

        if method.is_static {
            diagnostics.report_error(
                format!(
                    "'@{}' method '{}' must be a non-static instance method",
                    role.attr_name(),
                    method.name.text
                ),
                Some(method.name.position),
            );
            return;
        }
        if method.is_async {
            diagnostics.report_error(
                format!(
                    "'@{}' method '{}' cannot be async",
                    role.attr_name(),
                    method.name.text
                ),
                Some(method.name.position),
            );
            return;
        }
        if method.accessor.is_some() {
            diagnostics.report_error(
                format!(
                    "'@{}' cannot be applied to a property accessor",
                    role.attr_name()
                ),
                Some(method.name.position),
            );
            return;
        }

        let arity = method.parameters.len();
        let expected = role.expected_arity();
        if arity != expected {
            diagnostics.report_error(
                format!(
                    "'@{}' method '{}' must take {} parameter(s), but takes {}",
                    role.attr_name(),
                    method.name.text,
                    expected,
                    arity
                ),
                Some(method.name.position),
            );
            return;
        }

        match role {
            ProtocolRole::Get => {
                if matches!(method.return_type, None | Some(Type::Void)) {
                    diagnostics.report_error(
                        format!(
                            "'@get_indexer' method '{}' must return a non-void value",
                            method.name.text
                        ),
                        Some(method.name.position),
                    );
                    return;
                }
            }
            ProtocolRole::Iterator => {
                let ok = method
                    .return_type
                    .as_ref()
                    .map(|t| Self::resolve_struct_parts(t).is_some())
                    .unwrap_or(false);
                if !ok {
                    diagnostics.report_error(
                        format!(
                            "'@iterator' method '{}' must return an enumerator object (class/struct)",
                            method.name.text
                        ),
                        Some(method.name.position),
                    );
                    return;
                }
            }
            ProtocolRole::Next => {
                let ok = match &method.return_type {
                    Some(t) => match Self::resolve_struct_parts(t) {
                        Some((base, args)) => base == "Option" && args.len() == 1,
                        None => false,
                    },
                    None => false,
                };
                if !ok {
                    diagnostics.report_error(
                        format!(
                            "'@next' method '{}' must return Option<T>",
                            method.name.text
                        ),
                        Some(method.name.position),
                    );
                    return;
                }
            }
            ProtocolRole::Set => {}
        }

        let hooks = self
            .protocol_hooks
            .entry(target_type_str.to_string())
            .or_default();
        if hooks.slot(role).is_some() {
            diagnostics.report_error(
                format!(
                    "'{}' already declares an '@{}' method",
                    target_type_str,
                    role.attr_name()
                ),
                Some(method.name.position),
            );
            return;
        }
        *hooks.slot_mut(role) = Some(ProtocolHook {
            mangled_name: mangled_name.to_string(),
            surface_name: method.name.text.clone(),
        });
    }

    /// The registered protocol hook of `role` on `obj_type`, if any. Instantiates generic
    /// receivers first. Returns an owned clone so callers can keep making `&mut self` calls.
    pub(in crate::analyzer) fn protocol_hook(
        &mut self,
        obj_type: &Type,
        role: ProtocolRole,
        diagnostics: &mut DiagnosticBag,
    ) -> Option<ProtocolHook> {
        let (base_name, generic_args) = match Self::resolve_struct_parts(obj_type) {
            Some(parts) => {
                self.ensure_type_instantiated(&parts.0, &parts.1, &empty_span(), diagnostics);
                parts
            }
            None if matches!(obj_type, Type::String(_)) => ("string".to_string(), Vec::new()),
            None => return None,
        };
        let mono_name = dream_syntax::nodes::types::mangle_generic(&base_name, &generic_args);
        self.protocol_hooks
            .get(&mono_name)
            .and_then(|h| h.slot(role).clone())
    }
}
