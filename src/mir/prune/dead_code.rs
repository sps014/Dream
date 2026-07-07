//! The MIR reachability core: [`prune_module`] removes functions unreachable from the module's entry
//! points, then drops globals no surviving function reads.

use super::hir_edges::{hir_body_edges, HirEdges};
use super::FnKey;
use crate::mir::lower;
use crate::mir::{Global, Mir, Operand, Place, Rvalue, Statement, Terminator};
use crate::types::TypeId;
use std::collections::{HashMap, HashSet};

/// Records every callable this rvalue statically references (direct calls, first-class function
/// refs, and user constructors) into `out`.
fn rvalue_callees(rv: &Rvalue, out: &mut Vec<FnKey>) {
    match rv {
        Rvalue::Call { callee, .. } | Rvalue::FuncRef(callee) | Rvalue::JsCall { callee, .. } => {
            out.push((callee.def, callee.args.clone()))
        }
        Rvalue::New {
            ctor: Some(ctor), ..
        } => out.push((*ctor, vec![])),
        _ => {}
    }
}

/// Removes functions unreachable from the module's entry points, then tree-shakes the module's other
/// symbol tables. Dead pure stores to never-read globals are removed (then the now-unreferenced
/// globals are dropped). See [`prune_functions`] for the reachability core; the extra shaking lives
/// in [`prune_dead_globals`].
pub fn prune_module(mir: &mut Mir) {
    prune_functions(mir);
    prune_dead_globals(mir);
}

/// Removes functions unreachable from the module's entry points (the reachability core of
/// [`prune_module`]).
///
/// Reachability starts from `main` and the synthesized global initializer and follows direct calls,
/// `FuncRef`s, and constructors. An `IndirectCall` has no static target, but its only possible
/// targets are functions whose address was taken by a `FuncRef` in reachable code — which the
/// `FuncRef` edges already keep — so the result stays sound.
fn prune_functions(mir: &mut Mir) {
    let index: HashMap<FnKey, usize> = mir
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| ((f.def, f.instance.clone()), i))
        .collect();

    // `<Type>_del`/`<Type>_to_string` are invoked only by the generated RC runtime (the release
    // helpers and `$print_object`), never by a normal call edge, so reachability tracks them by name
    // for every type that is *live* — constructed (`New`/`UnionNew`) or printed — plus, transitively,
    // the types of its (reference) fields, whose release/print the runtime chains into.
    let by_name: HashMap<&str, usize> = mir
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();

    let mut reachable: HashSet<usize> = HashSet::new();
    let mut live_types: HashSet<TypeId> = HashSet::new();
    let mut type_worklist: Vec<TypeId> = Vec::new();
    let mut worklist: Vec<usize> = mir
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name == crate::mir::abi::ENTRY_FN || f.name == lower::INIT_FN_NAME)
        .map(|(i, _)| i)
        .collect();

    loop {
        while let Some(idx) = worklist.pop() {
            if !reachable.insert(idx) {
                continue;
            }
            let mut callees = Vec::new();
            let mut iface_uses: Vec<(usize, usize)> = Vec::new();
            for block in &mir.functions[idx].blocks {
                for stmt in &block.stmts {
                    match stmt {
                        Statement::Call { callee, .. } => {
                            callees.push((callee.def, callee.args.clone()))
                        }
                        Statement::InterfaceCall {
                            iface_id,
                            method_slot,
                            ..
                        } => iface_uses.push((*iface_id, *method_slot)),
                        Statement::Assign(_, rv) => {
                            rvalue_callees(rv, &mut callees);
                            if let Rvalue::InterfaceCall {
                                iface_id,
                                method_slot,
                                ..
                            } = rv
                            {
                                iface_uses.push((*iface_id, *method_slot));
                            }
                            match rv {
                                Rvalue::New { ty, .. } | Rvalue::UnionNew { ty, .. } => {
                                    type_worklist.push(*ty)
                                }
                                _ => {}
                            }
                        }
                        Statement::Print { ty, .. } => type_worklist.push(*ty),
                        _ => {}
                    }
                }
            }
            // An async function's MIR body is a stub; its real call/type edges live in the preserved
            // HIR snapshot, so walk that too (otherwise awaited helpers would be pruned).
            let f = &mir.functions[idx];
            if f.is_async {
                if let Some(hir_fn) = &f.hir_fn {
                    let mut edges = HirEdges::default();
                    hir_body_edges(&hir_fn.body, &mut edges);
                    callees.extend(edges.callees);
                    type_worklist.extend(edges.types);
                    iface_uses.extend(edges.iface_calls);
                }
            }
            for key in callees {
                if let Some(&target) = index.get(&key) {
                    if !reachable.contains(&target) {
                        worklist.push(target);
                    }
                }
            }
            // An interface call may dynamically reach the concrete method of *any* class that
            // implements that interface. Keep each such `{Class}_{method}` implementation alive
            // (by name, like the RC-runtime-only `_del`/`_to_string` helpers).
            for (iface_id, slot) in iface_uses {
                for imp in &mir.interfaces.impls {
                    for (id, symbols) in &imp.entries {
                        if *id == iface_id {
                            if let Some(sym) = symbols.get(slot) {
                                if let Some(&t) = by_name.get(sym.as_str()) {
                                    if !reachable.contains(&t) {
                                        worklist.push(t);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Expand the live-type frontier: keep each type's destructor/`to_string` and recurse into its
        // fields. Any newly-kept function is pushed back so its own body is walked; the outer loop
        // reaches a fixpoint once the function worklist drains and no new type is discovered.
        while let Some(ty) = type_worklist.pop() {
            if !live_types.insert(ty) {
                continue;
            }
            let mut field_tys = Vec::new();
            let mut names = Vec::new();
            if let Some(l) = mir.layouts.structs.get(&ty) {
                names.push(l.name.clone());
                field_tys.extend(l.fields.iter().map(|f| f.ty));
            }
            if let Some(l) = mir.layouts.unions.get(&ty) {
                names.push(l.name.clone());
                field_tys.extend(
                    l.variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|f| f.ty)),
                );
            }
            for name in names {
                for sym in [format!("{}_del", name), format!("{}_to_string", name)] {
                    if let Some(&idx) = by_name.get(sym.as_str()) {
                        if !reachable.contains(&idx) {
                            worklist.push(idx);
                        }
                    }
                }
            }
            type_worklist.extend(field_tys);
        }
        if worklist.is_empty() {
            break;
        }
    }
    drop(by_name);

    let mut keep = reachable.into_iter().collect::<Vec<_>>();
    keep.sort_unstable();
    let mut kept = Vec::with_capacity(keep.len());
    for (i, f) in std::mem::take(&mut mir.functions).into_iter().enumerate() {
        if keep.binary_search(&i).is_ok() {
            kept.push(f);
        }
    }
    mir.functions = kept;
}

/// Drops module globals that no surviving function reads. A global whose only writes are pure (no
/// call/allocation on the RHS) and which is never read is fully dead: its stores are removed and the
/// slot is dropped. A global written by an impure store (a call that may have side effects) is kept
/// even if never read, so the effect still runs. Globals are keyed by their stable `Global` id (the
/// backend emits `$g{id}` by id, not by position), so dropping entries never renumbers survivors.
fn prune_dead_globals(mir: &mut Mir) {
    let mut read: HashSet<Global> = HashSet::new();
    for f in &mir.functions {
        for b in &f.blocks {
            for s in &b.stmts {
                collect_global_reads_stmt(s, &mut read);
            }
            collect_global_reads_terminator(&b.terminator, &mut read);
        }
    }
    // Remove pure stores to never-read globals.
    for f in &mut mir.functions {
        for b in &mut f.blocks {
            b.stmts.retain(|s| match s {
                Statement::Assign(Place::Global(g), rv) => {
                    read.contains(g) || !crate::mir::passes::is_pure(rv)
                }
                _ => true,
            });
        }
    }
    // A global stays if it is still read or still written by a surviving (impure) store.
    let mut referenced = read;
    for f in &mir.functions {
        for b in &f.blocks {
            for s in &b.stmts {
                if let Statement::Assign(Place::Global(g), _) = s {
                    referenced.insert(*g);
                }
            }
        }
    }
    mir.globals.retain(|g| referenced.contains(&g.id));
}

fn collect_global_reads_stmt(s: &Statement, out: &mut HashSet<Global>) {
    match s {
        Statement::Assign(place, rv) => {
            if let Place::Index { index, .. } = place {
                collect_global_reads_operand(index, out);
            }
            collect_global_reads_rvalue(rv, out);
        }
        Statement::Retain(o) | Statement::Release(o) => collect_global_reads_operand(o, out),
        Statement::Call { args, .. } => args
            .iter()
            .for_each(|a| collect_global_reads_operand(a, out)),
        Statement::InterfaceCall { receiver, args, .. } => {
            collect_global_reads_operand(receiver, out);
            args.iter()
                .for_each(|a| collect_global_reads_operand(a, out));
        }
        Statement::Print { arg, .. } => collect_global_reads_operand(arg, out),
        Statement::Nop | Statement::DebugLine(_) => {}
    }
}

fn collect_global_reads_rvalue(rv: &Rvalue, out: &mut HashSet<Global>) {
    match rv {
        Rvalue::Select {
            cond,
            then_val,
            else_val,
        } => {
            collect_global_reads_operand(cond, out);
            collect_global_reads_operand(then_val, out);
            collect_global_reads_operand(else_val, out);
        }
        Rvalue::Use(o)
        | Rvalue::Unary(_, o)
        | Rvalue::ArrayLen(o)
        | Rvalue::StrLen(o)
        | Rvalue::Cast(o, _, _)
        | Rvalue::IsType(o, _)
        | Rvalue::Discriminant(o)
        | Rvalue::HashCode(o)
        | Rvalue::ToString(o)
        | Rvalue::EnumName { value: o, .. }
        | Rvalue::ArrayNew { len: o, .. }
        | Rvalue::ToBytes { value: o, .. }
        | Rvalue::FromBytes { bytes: o, .. }
        | Rvalue::UnionField { base: o, .. } => collect_global_reads_operand(o, out),
        Rvalue::Binary(_, a, b) | Rvalue::CharAt(a, b) | Rvalue::Concat(a, b) => {
            collect_global_reads_operand(a, out);
            collect_global_reads_operand(b, out);
        }
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::UnionNew { args, .. }
        | Rvalue::ArrayLit { elems: args, .. } => args
            .iter()
            .for_each(|a| collect_global_reads_operand(a, out)),
        Rvalue::IndirectCall { target, args } => {
            collect_global_reads_operand(target, out);
            args.iter()
                .for_each(|a| collect_global_reads_operand(a, out));
        }
        Rvalue::InterfaceCall { receiver, args, .. } => {
            collect_global_reads_operand(receiver, out);
            args.iter()
                .for_each(|a| collect_global_reads_operand(a, out));
        }
        Rvalue::JsCall {
            target,
            method,
            args,
            ..
        } => {
            collect_global_reads_operand(target, out);
            if let Some(m) = method {
                collect_global_reads_operand(m, out);
            }
            args.iter()
                .for_each(|(a, _)| collect_global_reads_operand(a, out));
        }
        Rvalue::FuncRef(_) => {}
    }
}

fn collect_global_reads_terminator(t: &Terminator, out: &mut HashSet<Global>) {
    match t {
        Terminator::If { cond, .. } => collect_global_reads_operand(cond, out),
        Terminator::Switch { value, .. } => collect_global_reads_operand(value, out),
        Terminator::Return(Some(o)) | Terminator::AsyncComplete(Some(o)) => {
            collect_global_reads_operand(o, out)
        }
        _ => {}
    }
}

fn collect_global_reads_operand(op: &Operand, out: &mut HashSet<Global>) {
    if let Operand::Copy(place) = op {
        match place {
            Place::Global(g) => {
                out.insert(*g);
            }
            Place::Index { index, .. } => collect_global_reads_operand(index, out),
            Place::Local(_) | Place::Field { .. } => {}
        }
    }
}
