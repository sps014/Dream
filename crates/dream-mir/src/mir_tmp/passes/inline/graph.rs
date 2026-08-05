//! Call-graph analysis backing the inliner's eligibility decisions: per-callee direct-call-site
//! counts, the address-taken set (functions reachable indirectly), and the recursive set (functions
//! on a call cycle, which are never inlined so the acyclic remainder terminates).

use super::FnKey;
use crate::mir::{Rvalue, Statement};
use std::collections::{HashMap, HashSet};

/// Counts direct-call sites (the forms inlining rewrites) per callee across the module.
pub(super) fn count_call_sites(mir: &crate::mir::Mir) -> HashMap<FnKey, usize> {
    let mut counts: HashMap<FnKey, usize> = HashMap::new();
    for f in &mir.functions {
        for b in &f.blocks {
            for s in &b.stmts {
                match s {
                    Statement::Call { callee, .. } => {
                        *counts.entry((callee.def, callee.args.clone())).or_default() += 1;
                    }
                    Statement::Assign(_, Rvalue::Call { callee, .. }) => {
                        *counts.entry((callee.def, callee.args.clone())).or_default() += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    counts
}

/// Functions whose address is taken (`FuncRef`): they may be reached indirectly, so they must not be
/// treated as single-use even if they have one direct call site.
pub(super) fn address_taken(mir: &crate::mir::Mir) -> HashSet<FnKey> {
    let mut set = HashSet::new();
    for f in &mir.functions {
        for b in &f.blocks {
            for s in &b.stmts {
                if let Statement::Assign(_, Rvalue::FuncRef(c)) = s {
                    set.insert((c.def, c.args.clone()));
                }
            }
        }
    }
    set
}

/// The set of functions that lie on a call cycle (an SCC of size > 1, or a self-loop). Inlining is
/// never attempted for these, guaranteeing termination over the remaining (acyclic) call graph.
pub(super) fn recursive_set(
    mir: &crate::mir::Mir,
    index: &HashMap<FnKey, usize>,
) -> HashSet<FnKey> {
    let n = mir.functions.len();
    // Adjacency over all statically visible call edges (including constructors + async HIR edges), so
    // any cyclic function is excluded even if the cycle runs through a non-inlined edge.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, f) in mir.functions.iter().enumerate() {
        let mut keys: Vec<FnKey> = Vec::new();
        for b in &f.blocks {
            for s in &b.stmts {
                match s {
                    Statement::Call { callee, .. } => keys.push((callee.def, callee.args.clone())),
                    Statement::Assign(_, rv) => stmt_rvalue_keys(rv, &mut keys),
                    _ => {}
                }
            }
        }
        if f.is_async {
            if let Some(hir_fn) = &f.hir_fn {
                let mut edges = crate::mir::HirEdges::default();
                crate::mir::hir_body_edges(&hir_fn.body, &mut edges);
                keys.extend(edges.callees);
            }
        }
        for k in keys {
            if let Some(&j) = index.get(&k) {
                adj[i].push(j);
            }
        }
    }
    let sccs = tarjan_scc(&adj);
    let mut recursive = HashSet::new();
    for scc in sccs {
        let cyclic = scc.len() > 1 || (scc.len() == 1 && adj[scc[0]].contains(&scc[0]));
        if cyclic {
            for node in scc {
                let f = &mir.functions[node];
                recursive.insert((f.def, f.instance.clone()));
            }
        }
    }
    recursive
}

fn stmt_rvalue_keys(rv: &Rvalue, out: &mut Vec<FnKey>) {
    match rv {
        Rvalue::Call { callee, .. } | Rvalue::FuncRef(callee) => {
            out.push((callee.def, callee.args.clone()))
        }
        Rvalue::New {
            ctor: Some(ctor), ..
        } => out.push((*ctor, vec![])),
        _ => {}
    }
}

/// Iterative Tarjan strongly-connected-components (iterative to avoid deep recursion on large call
/// graphs). Returns one `Vec<usize>` per SCC.
fn tarjan_scc(adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = adj.len();
    let mut index_of = vec![usize::MAX; n];
    let mut lowlink = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut sccs: Vec<Vec<usize>> = Vec::new();
    let mut next_index = 0usize;

    // Explicit DFS stack of (node, next-child-cursor).
    for start in 0..n {
        if index_of[start] != usize::MAX {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(v, ci)) = work.last() {
            if ci == 0 {
                index_of[v] = next_index;
                lowlink[v] = next_index;
                next_index += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if ci < adj[v].len() {
                let w = adj[v][ci];
                work.last_mut().unwrap().1 += 1;
                if index_of[w] == usize::MAX {
                    work.push((w, 0));
                } else if on_stack[w] {
                    lowlink[v] = lowlink[v].min(index_of[w]);
                }
            } else {
                if lowlink[v] == index_of[v] {
                    let mut scc = Vec::new();
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack[w] = false;
                        scc.push(w);
                        if w == v {
                            break;
                        }
                    }
                    sccs.push(scc);
                }
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    lowlink[parent] = lowlink[parent].min(lowlink[v]);
                }
            }
        }
    }
    sccs
}
