//! Compile-time detection of ARC reference cycles between `class` types, plus validation of the
//! `weak`/`unowned` field modifiers that break them.
//!
//! [`Analyzer::check_reference_cycles`] builds a directed graph whose nodes are non-generic
//! `class` declarations and whose edges are strong (non-`weak`/`unowned`) fields that hold (or,
//! through `Option<T>`/`T[]`, transitively hold) a reference to another class. Any strongly
//! connected component of that graph — including a self-loop — is a leak the runtime's ARC can
//! never collect, and is reported as a hard compile error unless every class in the cycle carries
//! `@allow_cycle`. See `docs/language/memory.md` and the design note referenced there for the
//! full rationale, including why this is a *structural* check (it cannot see cycles assembled
//! dynamically through collections/`object`/callbacks).

use super::*;
use indexmap::IndexMap;

/// One strong-reference edge in the class reference-cycle graph: field `field_name` (declared at
/// `field_position`) of the owning class holds a class-typed value of `to`.
struct ClassEdge {
    field_name: String,
    field_position: Option<TextSpan>,
    to: String,
}

impl<'a> Analyzer<'a> {
    /// Validates every `weak`/`unowned` field's shape, then runs the whole-program class
    /// reference-cycle check. Called from [`Self::register_structs`] once every non-generic class
    /// is registered in `self.struct_table` (so field types can be classified as value/reference).
    pub(in crate::analyzer) fn check_weak_unowned_and_cycles(
        &self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        self.validate_weak_unowned_fields(node, diagnostics);
        self.check_reference_cycles(node, diagnostics);
    }

    /// `weak` fields must be `Option<T>` for a class `T`; `unowned` fields must themselves be a
    /// bare class type `T`; a field cannot be both.
    fn validate_weak_unowned_fields(
        &self,
        node: &'a ProgramNode<'a>,
        diagnostics: &mut DiagnosticBag,
    ) {
        for struct_decl in node.structs.iter() {
            for field in &struct_decl.fields {
                if !field.is_weak && !field.is_unowned {
                    continue;
                }
                if field.is_weak && field.is_unowned {
                    diagnostics.report_error(
                        format!(
                            "field '{}' cannot be both 'weak' and 'unowned'",
                            field.name.text
                        ),
                        Some(field.name.position),
                    );
                    continue;
                }
                if field.is_weak {
                    let option_inner = match &field.field_type {
                        Type::Struct(token, Some(args))
                            if token.text == "Option" && args.len() == 1 =>
                        {
                            Some(&args[0])
                        }
                        _ => None,
                    };
                    let is_class_option = option_inner
                        .and_then(Self::resolve_struct_parts)
                        .and_then(|(base, _)| {
                            self.struct_table.get_struct(&base).map(|i| !i.is_value)
                        })
                        .unwrap_or(false);
                    if !is_class_option {
                        diagnostics.report_error(
                            format!(
                                "'weak' field '{}' must have type 'Option<T>' where 'T' is a class, got '{}'",
                                field.name.text,
                                field.field_type.display_name()
                            ),
                            Some(field.name.position),
                        );
                    }
                } else {
                    let is_class = Self::resolve_struct_parts(&field.field_type)
                        .and_then(|(base, _)| {
                            self.struct_table.get_struct(&base).map(|i| !i.is_value)
                        })
                        .unwrap_or(false);
                    if !is_class {
                        diagnostics.report_error(
                            format!(
                                "'unowned' field '{}' must have a class type, got '{}'",
                                field.name.text,
                                field.field_type.display_name()
                            ),
                            Some(field.name.position),
                        );
                    }
                }
            }
        }
    }

    /// Builds the strong-reference graph over non-generic `class` declarations and hard-errors on
    /// every strongly connected component (Tarjan's SCC), unless every class in it carries
    /// `@allow_cycle`.
    fn check_reference_cycles(&self, node: &'a ProgramNode<'a>, diagnostics: &mut DiagnosticBag) {
        let mut edges: IndexMap<String, Vec<ClassEdge>> = IndexMap::new();
        let mut allow_cycle: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut positions: std::collections::HashMap<String, TextSpan> =
            std::collections::HashMap::new();

        for struct_decl in node.structs.iter() {
            // Generic templates aren't monomorphized here (their field types aren't concrete
            // yet); value structs already have their own containment check (they can't hold a
            // strong cycle at all, since embedding a class only ever takes a reference).
            if struct_decl.generic_parameters.is_some() || struct_decl.is_value {
                continue;
            }
            let name = struct_decl.name.text.clone();
            positions.insert(name.clone(), struct_decl.name.position);
            if struct_decl
                .attributes
                .iter()
                .any(|a| a.name.text == "allow_cycle")
            {
                allow_cycle.insert(name.clone());
            }

            let mut out = Vec::new();
            for field in &struct_decl.fields {
                if field.is_weak || field.is_unowned {
                    continue;
                }
                for target in self.strong_ref_targets(&field.field_type) {
                    out.push(ClassEdge {
                        field_name: field.name.text.clone(),
                        field_position: Some(field.name.position),
                        to: target,
                    });
                }
            }
            edges.entry(name).or_default().extend(out);
        }

        let nodes: Vec<String> = edges.keys().cloned().collect();
        let sccs = tarjan_scc(&nodes, &edges);

        for scc in sccs {
            let self_loop = scc.len() == 1
                && edges
                    .get(&scc[0])
                    .map(|es| es.iter().any(|e| e.to == scc[0]))
                    .unwrap_or(false);
            if scc.len() < 2 && !self_loop {
                continue;
            }
            // `@allow_cycle` only suppresses a cycle when *every* class participating in it opted
            // in; a cycle merely passing through one annotated class among several is still an
            // error, so the escape hatch can't be laundered onto a multi-class cycle.
            if scc.iter().all(|c| allow_cycle.contains(c)) {
                continue;
            }

            let scc_set: std::collections::HashSet<&str> = scc.iter().map(String::as_str).collect();
            let mut culprits: Vec<(String, Option<TextSpan>)> = Vec::new();
            for class in &scc {
                if let Some(es) = edges.get(class) {
                    for e in es {
                        if scc_set.contains(e.to.as_str()) {
                            culprits.push((
                                format!("'{}.{}'", class, e.field_name),
                                e.field_position.or_else(|| positions.get(class).copied()),
                            ));
                        }
                    }
                }
            }
            culprits.sort_by(|a, b| a.0.cmp(&b.0));
            culprits.dedup_by(|a, b| a.0 == b.0);
            let position = culprits.first().and_then(|c| c.1);
            let list = culprits
                .iter()
                .map(|c| c.0.as_str())
                .collect::<Vec<_>>()
                .join(", ");

            diagnostics.report_error(
                format!(
                    "reference cycle detected: {} form a strong-reference cycle, so none of their objects can ever be freed; mark one field 'weak' or 'unowned' to break it, or annotate every class in the cycle with '@allow_cycle' if the cycle is intentional",
                    list
                ),
                position,
            );
        }
    }

    /// The class names transitively strong-referenced by `ty`: `ty` itself if it names a class,
    /// or (recursively) the element type of `T[]` / the payload type of `Option<T>`. Anything else
    /// (primitives, value structs, generic collections other than `Option`, unresolved types)
    /// contributes no edge — a known, documented limitation of this structural check.
    fn strong_ref_targets(&self, ty: &Type) -> Vec<String> {
        match ty {
            Type::Array(inner) => self.strong_ref_targets(inner),
            Type::Struct(token, args) => {
                if token.text == "Option" {
                    return match args {
                        Some(a) if a.len() == 1 => self.strong_ref_targets(&a[0]),
                        _ => Vec::new(),
                    };
                }
                match self.struct_table.get_struct(&token.text) {
                    Some(info) if !info.is_value => vec![token.text.clone()],
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }
}

/// Tarjan's strongly-connected-components algorithm over the class strong-reference graph.
/// Returns every SCC (including singletons with no self-loop, which callers filter out).
fn tarjan_scc(nodes: &[String], edges: &IndexMap<String, Vec<ClassEdge>>) -> Vec<Vec<String>> {
    #[allow(clippy::too_many_arguments)]
    fn strongconnect(
        v: &str,
        edges: &IndexMap<String, Vec<ClassEdge>>,
        index_counter: &mut usize,
        stack: &mut Vec<String>,
        indices: &mut std::collections::HashMap<String, usize>,
        lowlink: &mut std::collections::HashMap<String, usize>,
        on_stack: &mut std::collections::HashMap<String, bool>,
        result: &mut Vec<Vec<String>>,
    ) {
        let idx = *index_counter;
        indices.insert(v.to_string(), idx);
        lowlink.insert(v.to_string(), idx);
        *index_counter += 1;
        stack.push(v.to_string());
        on_stack.insert(v.to_string(), true);

        if let Some(es) = edges.get(v) {
            for e in es {
                let w = e.to.as_str();
                if !indices.contains_key(w) {
                    strongconnect(
                        w,
                        edges,
                        index_counter,
                        stack,
                        indices,
                        lowlink,
                        on_stack,
                        result,
                    );
                    let w_low = lowlink[w];
                    let v_low = lowlink.get_mut(v).unwrap();
                    *v_low = (*v_low).min(w_low);
                } else if *on_stack.get(w).unwrap_or(&false) {
                    let w_idx = indices[w];
                    let v_low = lowlink.get_mut(v).unwrap();
                    *v_low = (*v_low).min(w_idx);
                }
            }
        }

        if lowlink[v] == indices[v] {
            let mut component = Vec::new();
            loop {
                let w = stack.pop().unwrap();
                on_stack.insert(w.clone(), false);
                let done = w == v;
                component.push(w);
                if done {
                    break;
                }
            }
            result.push(component);
        }
    }

    let mut index_counter = 0usize;
    let mut stack = Vec::new();
    let mut indices = std::collections::HashMap::new();
    let mut lowlink = std::collections::HashMap::new();
    let mut on_stack = std::collections::HashMap::new();
    let mut result = Vec::new();

    for n in nodes {
        if !indices.contains_key(n.as_str()) {
            strongconnect(
                n,
                edges,
                &mut index_counter,
                &mut stack,
                &mut indices,
                &mut lowlink,
                &mut on_stack,
                &mut result,
            );
        }
    }

    result
}
