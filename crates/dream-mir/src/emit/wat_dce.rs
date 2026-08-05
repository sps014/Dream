//! Whole-module dead-function elimination on the emitted WAT text.
//!
//! The backend embeds a fixed runtime (allocator, strings, object protocol, formatters, async
//! scheduler) verbatim in every module. Most programs use only a slice of it. This pass builds the
//! call graph among the module's functions and drops every function not transitively reachable from
//! the module's roots (exports, `(start …)`, and the function-table `(elem …)` entries).
//!
//! Reachability is computed **structurally** from the real WAT AST (`wast`): each function's call
//! edges come from its actual `call` / `return_call` / `ref.func` instructions — not from a
//! heuristic scan for `$tokens`, which over-approximated (any `$name` appearing anywhere, even
//! inside a string literal or comment, kept the function alive). Precise edges mean tighter
//! trimming.
//!
//! The AST is used only to decide *which* function definitions are live; the surviving text is
//! spliced from the original module by top-level item byte-ranges, so formatting is preserved
//! exactly. Imports, memory, globals, types, tables, data, and every non-`func` item are always
//! kept, so the module stays structurally valid; `wat::parse_str` in the driver is the final
//! correctness gate. If the module cannot be parsed as a single core module, the input is returned
//! unchanged (no trimming — always sound).

use std::collections::{HashMap, HashSet, VecDeque};
use wast::core::{ElemPayload, Func, FuncKind, Instruction, ModuleField, ModuleKind};
use wast::parser::{self, ParseBuffer};
use wast::token::Index;
use wast::Wat;

/// Removes unreachable `(func …)` definitions from a complete WAT module. Returns the input
/// unchanged if it does not parse as a single `(module …)` with at least one top-level item, or if
/// structural reachability cannot be computed.
pub(super) fn strip_dead_functions(module: &str) -> String {
    let b = module.as_bytes();
    let Some((prefix_end, items, outer_close)) = parse_module(b) else {
        return module.to_string();
    };
    if items.is_empty() {
        return module.to_string();
    }

    // Structural reachability over the real AST. On any parse failure, keep everything (sound).
    let Some(live) = live_function_names(module) else {
        return module.to_string();
    };

    // Rebuild: keep every non-func item, every anonymous func (may be referenced by index or is an
    // exported shim), and every reachable named func — preserving order.
    let mut out = String::with_capacity(module.len());
    out.push_str(&module[..prefix_end]);
    let mut first = true;
    for &(s, e) in &items {
        let text = &module[s..e];
        let keep = if is_func_item(text) {
            match func_name(text) {
                // A named function survives iff it is reachable from a root.
                Some(name) => live.contains(name.trim_start_matches('$')),
                // Anonymous functions (e.g. the exported `main` shim) can't be named as dead, so
                // keep them — there is at most a handful and they are always roots anyway.
                None => true,
            }
        } else {
            true
        };
        if !keep {
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(text);
    }
    out.push('\n');
    out.push_str(&module[outer_close..]);
    out
}

/// Parses `module` with the `wast` AST and returns the set of function names (without the leading
/// `$`) transitively reachable from the module roots. Returns `None` if the text does not parse as
/// a single core module.
fn live_function_names(module: &str) -> Option<HashSet<String>> {
    let buf = ParseBuffer::new(module).ok()?;
    let wat = parser::parse::<Wat>(&buf).ok()?;
    let Wat::Module(m) = wat else { return None };
    let ModuleKind::Text(fields) = &m.kind else {
        return None;
    };

    // Call graph over named functions, plus the initial root set.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();

    for field in fields {
        match field {
            ModuleField::Func(f) => {
                if let Some(name) = func_id(f) {
                    edges.insert(name.clone(), func_refs(f));
                    // Inline `(func $x (export "x") …)` is itself a root.
                    if !f.exports.names.is_empty() {
                        roots.push(name);
                    }
                }
            }
            ModuleField::Export(e) => {
                if let Index::Id(id) = &e.item {
                    roots.push(id.name().to_string());
                }
            }
            ModuleField::Start(Index::Id(id)) => roots.push(id.name().to_string()),
            ModuleField::Elem(elem) => match &elem.payload {
                ElemPayload::Indices(indices) => {
                    for idx in indices {
                        if let Index::Id(id) = idx {
                            roots.push(id.name().to_string());
                        }
                    }
                }
                ElemPayload::Exprs { exprs, .. } => {
                    for expr in exprs {
                        collect_expr_refs(&expr.instrs, &mut roots);
                    }
                }
            },
            _ => {}
        }
    }

    // BFS over the call graph from the roots.
    let mut live: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for r in roots {
        if live.insert(r.clone()) {
            queue.push_back(r);
        }
    }
    while let Some(name) = queue.pop_front() {
        let Some(refs) = edges.get(&name) else {
            continue;
        };
        for r in refs.clone() {
            if live.insert(r.clone()) {
                queue.push_back(r);
            }
        }
    }
    Some(live)
}

/// The `$name` (without `$`) a function is defined under, or `None` for an anonymous function.
fn func_id(f: &Func) -> Option<String> {
    f.id.map(|id| id.name().to_string())
}

/// The names of every function this function references via `call` / `return_call` / `ref.func`.
fn func_refs(f: &Func) -> Vec<String> {
    let mut refs = Vec::new();
    if let FuncKind::Inline { expression, .. } = &f.kind {
        collect_expr_refs(&expression.instrs, &mut refs);
    }
    refs
}

/// Appends every function name referenced by these (flattened) instructions. `call_indirect` /
/// `return_call_indirect` reference a *type*, not a function, so they are intentionally ignored.
fn collect_expr_refs(instrs: &[Instruction], out: &mut Vec<String>) {
    for instr in instrs {
        if let Instruction::Call(Index::Id(id))
        | Instruction::ReturnCall(Index::Id(id))
        | Instruction::RefFunc(Index::Id(id)) = instr
        {
            out.push(id.name().to_string());
        }
    }
}

/// `(byte offset of the first top-level item, the byte ranges of every top-level list item, the
/// byte offset of the outer closing paren)`.
type ModuleItems = (usize, Vec<(usize, usize)>, usize);

/// Parses the outer `(module …)`, splitting it into its top-level items.
fn parse_module(b: &[u8]) -> Option<ModuleItems> {
    let n = b.len();
    let mut i = skip_trivia(b, 0);
    if i >= n || b[i] != b'(' {
        return None;
    }
    i += 1; // past outer '('
    i = skip_trivia(b, i);
    // Expect the `module` keyword atom.
    if !b[i..].starts_with(b"module") {
        return None;
    }
    i = skip_atom(b, i);

    let mut items: Vec<(usize, usize)> = Vec::new();
    let mut first_start = None;
    loop {
        i = skip_trivia(b, i);
        if i >= n {
            return None;
        }
        match b[i] {
            b')' => return Some((first_start.unwrap_or(i), items, i)),
            b'(' => {
                let start = i;
                let end = read_list(b, i);
                if first_start.is_none() {
                    first_start = Some(start);
                }
                items.push((start, end));
                i = end;
            }
            _ => i = skip_atom(b, i),
        }
    }
}

/// Extracts the `$name` of a `(func …)` or `(import … (func …))` item, or `None` for an anonymous function.
fn func_name(item: &str) -> Option<String> {
    let b = item.as_bytes();
    let n = b.len();
    let mut i = skip_trivia(b, 0);
    if i >= n || b[i] != b'(' {
        return None;
    }
    i += 1;
    i = skip_trivia(b, i);
    let start = i;
    i = skip_atom(b, i);
    let keyword = &b[start..i];

    if keyword == b"import" {
        i = skip_trivia(b, i);
        if i < n && b[i] == b'"' {
            i = skip_string(b, i);
        }
        i = skip_trivia(b, i);
        if i < n && b[i] == b'"' {
            i = skip_string(b, i);
        }
        i = skip_trivia(b, i);
        if i < n && b[i] == b'(' {
            i += 1;
            i = skip_trivia(b, i);
            let start = i;
            i = skip_atom(b, i);
            if &b[start..i] != b"func" {
                return None;
            }
        } else {
            return None;
        }
    } else if keyword != b"func" {
        return None;
    }

    i = skip_trivia(b, i);
    if i < n && b[i] == b'$' {
        let start = i;
        let end = skip_atom(b, i);
        Some(item[start..end].to_string())
    } else {
        None
    }
}

/// Returns true if this top-level list is a `(func …)` or `(import … (func …))` definition.
fn is_func_item(text: &str) -> bool {
    let b = text.as_bytes();
    let n = b.len();
    let mut i = skip_trivia(b, 0);
    if i >= n || b[i] != b'(' {
        return false;
    }
    i += 1;
    i = skip_trivia(b, i);
    let start = i;
    i = skip_atom(b, i);
    let keyword = &b[start..i];

    if keyword == b"func" {
        return true;
    }
    if keyword == b"import" {
        i = skip_trivia(b, i);
        if i < n && b[i] == b'"' {
            i = skip_string(b, i);
        }
        i = skip_trivia(b, i);
        if i < n && b[i] == b'"' {
            i = skip_string(b, i);
        }
        i = skip_trivia(b, i);
        if i < n && b[i] == b'(' {
            i += 1;
            i = skip_trivia(b, i);
            let start = i;
            i = skip_atom(b, i);
            if &b[start..i] == b"func" {
                return true;
            }
        }
    }
    false
}

/// Reads a balanced `( … )` list starting at `b[start] == '('`; returns the index just past the
/// matching close paren. Respects string literals and `;;` / `(; ;)` comments.
fn read_list(b: &[u8], start: usize) -> usize {
    let n = b.len();
    let mut i = start;
    let mut depth = 0usize;
    while i < n {
        match b[i] {
            b'"' => i = skip_string(b, i),
            b';' if i + 1 < n && b[i + 1] == b';' => {
                i += 2;
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' if i + 1 < n && b[i + 1] == b';' => i = skip_block_comment(b, i),
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    n
}

fn skip_trivia(b: &[u8], mut i: usize) -> usize {
    let n = b.len();
    loop {
        if i >= n {
            return i;
        }
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b';' if i + 1 < n && b[i + 1] == b';' => {
                i += 2;
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' if i + 1 < n && b[i + 1] == b';' => i = skip_block_comment(b, i),
            _ => return i,
        }
    }
}

/// Skips a (possibly nested) `(; … ;)` block comment starting at `b[i] == '('`.
fn skip_block_comment(b: &[u8], mut i: usize) -> usize {
    let n = b.len();
    i += 2; // past `(;`
    let mut depth = 1;
    while i < n && depth > 0 {
        if i + 1 < n && b[i] == b'(' && b[i + 1] == b';' {
            depth += 1;
            i += 2;
        } else if i + 1 < n && b[i] == b';' && b[i + 1] == b')' {
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    i
}

fn skip_string(b: &[u8], start: usize) -> usize {
    let n = b.len();
    let mut i = start + 1;
    while i < n {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    n
}

/// Skips a bare atom/token (up to the next whitespace or paren), honoring embedded strings.
fn skip_atom(b: &[u8], mut i: usize) -> usize {
    let n = b.len();
    while i < n {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' => return i,
            b'"' => i = skip_string(b, i),
            _ => i += 1,
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_unreferenced_function() {
        let m = "(module\n\
                 (func $used (result i32) i32.const 1)\n\
                 (func $dead (result i32) i32.const 2)\n\
                 (func (export \"main\") (result i32) call $used)\n\
                 )\n";
        let out = strip_dead_functions(m);
        assert!(out.contains("$used"), "used func kept:\n{}", out);
        assert!(
            out.contains("export \"main\""),
            "exported shim kept:\n{}",
            out
        );
        assert!(!out.contains("$dead"), "dead func removed:\n{}", out);
    }

    #[test]
    fn keeps_transitively_reachable() {
        let m = "(module\n\
                 (func $a (result i32) call $b)\n\
                 (func $b (result i32) i32.const 7)\n\
                 (func $c (result i32) i32.const 9)\n\
                 (export \"a\" (func $a))\n\
                 )\n";
        let out = strip_dead_functions(m);
        assert!(
            out.contains("$a") && out.contains("$b"),
            "closure kept:\n{}",
            out
        );
        assert!(!out.contains("$c"), "dead func removed:\n{}", out);
    }

    #[test]
    fn elem_entries_are_roots() {
        let m = "(module\n\
                 (table $__ft 1 funcref)\n\
                 (elem (i32.const 0) $indirect)\n\
                 (func $indirect (result i32) i32.const 5)\n\
                 (func $dead (result i32) i32.const 6)\n\
                 )\n";
        let out = strip_dead_functions(m);
        assert!(out.contains("$indirect"), "table entry kept:\n{}", out);
        assert!(!out.contains("$dead"), "dead func removed:\n{}", out);
    }

    #[test]
    fn inline_export_is_root() {
        let m = "(module\n\
                 (func $keep (export \"keep\") (result i32) call $helper)\n\
                 (func $helper (result i32) i32.const 3)\n\
                 (func $gone (result i32) i32.const 4)\n\
                 )\n";
        let out = strip_dead_functions(m);
        assert!(out.contains("$keep"), "inline-exported kept:\n{}", out);
        assert!(out.contains("$helper"), "callee of export kept:\n{}", out);
        assert!(!out.contains("$gone"), "dead func removed:\n{}", out);
    }

    #[test]
    fn name_only_in_string_literal_is_not_a_reference() {
        // The old `$token` scanner kept `$ghost` alive because its name appears inside a data-style
        // string literal. Structural parsing sees no real `call`, so it must be dropped.
        let m = "(module\n\
                 (func (export \"main\") (result i32)\n\
                   i32.const 0\n\
                   drop\n\
                   i32.const 1)\n\
                 (func $ghost (result i32) i32.const 42)\n\
                 (data (i32.const 0) \"call $ghost\")\n\
                 )\n";
        let out = strip_dead_functions(m);
        assert!(
            !out.contains("func $ghost"),
            "func referenced only inside a string literal must be dropped:\n{}",
            out
        );
        // The data segment (a non-func item) is always preserved verbatim.
        assert!(out.contains("\"call $ghost\""), "data kept:\n{}", out);
    }

    #[test]
    fn name_only_in_comment_is_not_a_reference() {
        let m = "(module\n\
                 (func (export \"main\") (result i32) i32.const 1)\n\
                 (func $commented (result i32) i32.const 2) ;; call $commented later maybe\n\
                 )\n";
        let out = strip_dead_functions(m);
        assert!(
            !out.contains("func $commented"),
            "func mentioned only in a comment must be dropped:\n{}",
            out
        );
    }
}
