//! Handlers for the read-only DAP requests: `threads`, `setBreakpoints`, `stackTrace`, `scopes`,
//! `variables`, and `evaluate`. Split out of `mod.rs`.

use super::state::{Shared, ThreadState, VarValue, VAR_REF_BASE};
use serde_json::{json, Value};
use sourcemap::SourceMap;
use std::sync::Arc;

use super::sourcemap;

/// Lists every live execution thread. Falls back to a lone `main` before execution has started.
pub(super) fn handle_threads(shared: &Arc<Shared>) -> Value {
    let inner = shared.inner.lock().unwrap();
    let mut threads: Vec<(u32, String)> = inner
        .threads
        .iter()
        .map(|(id, t)| (*id, t.name.clone()))
        .collect();
    drop(inner);
    if threads.is_empty() {
        threads.push((super::MAIN_THREAD, "main".to_string()));
    }
    threads.sort_by_key(|(id, _)| *id);
    let list: Vec<Value> = threads
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    json!({ "threads": list })
}

pub(super) fn handle_set_breakpoints(msg: &Value, shared: &Arc<Shared>, sm: &SourceMap) -> Value {
    let path = msg
        .pointer("/arguments/source/path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let lines: Vec<u32> = msg
        .pointer("/arguments/breakpoints")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|b| b.get("line").and_then(|l| l.as_u64()).map(|l| l as u32))
                .collect()
        })
        .unwrap_or_default();

    let file_id = sm.file_id_for_path(path);
    let mut inner = shared.inner.lock().unwrap();
    // Replace all breakpoints for this file with the new set.
    if let Some(fid) = file_id {
        inner.breakpoints.retain(|(f, _)| *f != fid);
        for &line in &lines {
            inner.breakpoints.insert((fid, line));
        }
    }
    // Refresh the lock-free filter the per-statement line hook probes.
    shared.bp_filter.rebuild(&inner.breakpoints);
    drop(inner);

    let verified = file_id.is_some();
    let breakpoints: Vec<Value> = lines
        .iter()
        .map(|l| json!({ "verified": verified, "line": l }))
        .collect();
    json!({ "breakpoints": breakpoints })
}

pub(super) fn handle_stack_trace(shared: &Arc<Shared>, sm: &SourceMap, thread_id: u32) -> Value {
    let inner = shared.inner.lock().unwrap();
    let Some(t) = inner.threads.get(&thread_id) else {
        return json!({ "stackFrames": [], "totalFrames": 0 });
    };
    let base = ThreadState::base_ref(thread_id);
    // DAP wants the innermost frame first; our call stack has the outermost first. Frame ids are
    // namespaced per thread (`base + index`) so `scopes`/`variables` never collide across threads.
    let frames: Vec<Value> = t
        .call_stack
        .iter()
        .rev()
        .enumerate()
        .map(|(i, f)| {
            let name = sm
                .function(f.func_id)
                .map(|fi| fi.name.clone())
                .unwrap_or_else(|| format!("fn#{}", f.func_id));
            let mut frame = json!({
                "id": base + i as i64,
                "name": name,
                "line": f.line,
                "column": 1,
            });
            if let Some(path) = sm.file_path(f.file) {
                frame["source"] = json!({
                    "name": std::path::Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(path),
                    "path": path,
                });
            }
            frame
        })
        .collect();
    let total = frames.len();
    json!({ "stackFrames": frames, "totalFrames": total })
}

/// Resolves a frame's Locals scope. A frame id is `thread_base + frame_index`; only the innermost
/// frame (index 0, i.e. `frame_id % VAR_REF_BASE == 0`) has live locals — its `variablesReference` is
/// the thread's base. Non-top frames return 0 (their spilled globals have been overwritten).
pub(super) fn scopes_body(frame_id: i64) -> Value {
    let reference = if frame_id > 0 && frame_id % VAR_REF_BASE == 0 {
        frame_id
    } else {
        0
    };
    json!({
        "scopes": [
            {
                "name": "Locals",
                "variablesReference": reference,
                "expensive": false,
            }
        ]
    })
}

pub(super) fn handle_variables(shared: &Arc<Shared>, reference: i64) -> Value {
    // The owning thread is recovered from the reference's high part (see `VAR_REF_BASE`).
    let thread_id = (reference / VAR_REF_BASE) as u32;
    let inner = shared.inner.lock().unwrap();
    let variables: Vec<Value> = inner
        .threads
        .get(&thread_id)
        .and_then(|t| t.var_refs.get(&reference))
        .map(|vs| vs.iter().map(var_to_json).collect())
        .unwrap_or_default();
    json!({ "variables": variables })
}

fn var_to_json(v: &VarValue) -> Value {
    json!({
        "name": v.name,
        "value": v.value,
        "type": v.type_name,
        "variablesReference": v.variables_reference,
    })
}

pub(super) fn handle_evaluate(msg: &Value, shared: &Arc<Shared>) -> Value {
    let expr = msg
        .pointer("/arguments/expression")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    // Evaluate against the requested frame's thread (default: main). `frameId` is `thread_base + idx`.
    let frame_id = msg
        .pointer("/arguments/frameId")
        .and_then(|v| v.as_i64())
        .unwrap_or(VAR_REF_BASE);
    let thread_id = (frame_id / VAR_REF_BASE).max(1) as u32;
    let base = ThreadState::base_ref(thread_id);

    let inner = shared.inner.lock().unwrap();
    let Some(t) = inner.threads.get(&thread_id) else {
        return json!({ "result": "<not available>", "variablesReference": 0 });
    };
    // Support simple dotted field paths (`a.field.sub`) by walking the decoded variable tree.
    let mut parts = expr.split('.');
    let Some(head) = parts.next() else {
        return json!({ "result": "<not available>", "variablesReference": 0 });
    };
    let mut current = t
        .var_refs
        .get(&base)
        .and_then(|top| top.iter().find(|v| v.name == head));
    for seg in parts {
        current = match current {
            Some(v) if v.variables_reference != 0 => t
                .var_refs
                .get(&v.variables_reference)
                .and_then(|children| children.iter().find(|c| c.name == seg)),
            _ => None,
        };
    }
    match current {
        Some(v) => json!({
            "result": v.value,
            "type": v.type_name,
            "variablesReference": v.variables_reference,
        }),
        None => json!({ "result": "<not available>", "variablesReference": 0 }),
    }
}
