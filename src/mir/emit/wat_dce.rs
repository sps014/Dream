//! Whole-module dead-function elimination on the emitted WAT text.
//!
//! The backend embeds a fixed runtime (allocator, strings, object protocol, formatters, async
//! scheduler) verbatim in every module. Most programs use only a slice of it. This pass parses the
//! finished `(module …)` text, builds the call graph among its `(func …)` definitions, and drops
//! every function not transitively reachable from the module's roots (exports, `(start …)`, and the
//! function-table `(elem …)` entries).
//!
//! It is deliberately conservative — it never *adds* references and treats every `$name` token in a
//! kept function's body that matches a defined function as a reference, so it over-keeps rather than
//! risk dropping something live. Imports, memory, globals, types, tables, data, and every non-`func`
//! item are always preserved, so the module stays structurally valid; `wat::parse_str` in the driver
//! is the final correctness gate.

use std::collections::{HashMap, HashSet, VecDeque};

/// Removes unreachable `(func …)` definitions from a complete WAT module. Returns the input
/// unchanged if it does not parse as a single `(module …)` with at least one top-level item.
pub(super) fn strip_dead_functions(module: &str) -> String {
    let b = module.as_bytes();
    let Some((prefix_end, items, outer_close)) = parse_module(b) else {
        return module.to_string();
    };
    if items.is_empty() {
        return module.to_string();
    }

    // Classify items and index functions by name.
    struct Item {
        text: String,
        is_func: bool,
        name: Option<String>,
        force_keep: bool,
        refs: Vec<String>,
    }
    let mut parsed: Vec<Item> = Vec::with_capacity(items.len());
    let mut func_names: HashSet<String> = HashSet::new();
    for &(s, e) in &items {
        let text = &module[s..e];
        let is_func = text.trim_start().starts_with("(func");
        let name = if is_func { func_name(text) } else { None };
        if let Some(n) = &name {
            func_names.insert(n.clone());
        }
        parsed.push(Item {
            text: text.to_string(),
            is_func,
            name,
            force_keep: false,
            refs: Vec::new(),
        });
    }

    // Resolve references (dollar tokens that name a defined function) and roots.
    let mut name_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, it) in parsed.iter().enumerate() {
        if let Some(n) = &it.name {
            name_to_idx.insert(n.clone(), i);
        }
    }
    let mut root_names: Vec<String> = Vec::new();
    for it in &mut parsed {
        let tokens: Vec<String> = dollar_tokens(&it.text)
            .into_iter()
            .filter(|t| func_names.contains(t))
            .collect();
        if it.is_func {
            // A function exported inline (`(func $x (export "x") …)`) or an anonymous exported shim
            // is a root; everything it calls must be kept.
            it.force_keep = it.text.contains("(export");
            it.refs = tokens;
        } else {
            // export / start / elem / anything else: its function tokens are roots. Non-func items
            // are always kept, so we don't store them as refs, only as roots.
            root_names.extend(tokens);
        }
    }

    // Reachability over function items.
    let mut kept: HashSet<usize> = HashSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, it) in parsed.iter().enumerate() {
        if it.is_func && it.force_keep {
            if kept.insert(i) {
                queue.push_back(i);
            }
        }
    }
    for n in root_names {
        if let Some(&i) = name_to_idx.get(&n) {
            if kept.insert(i) {
                queue.push_back(i);
            }
        }
    }
    while let Some(i) = queue.pop_front() {
        // Clone refs to avoid borrowing `parsed` while mutating `kept`/`queue`.
        let refs = parsed[i].refs.clone();
        for r in refs {
            if let Some(&j) = name_to_idx.get(&r) {
                if kept.insert(j) {
                    queue.push_back(j);
                }
            }
        }
    }

    // Rebuild: keep every non-func item and every reachable func, preserving order.
    let mut out = String::with_capacity(module.len());
    out.push_str(&module[..prefix_end]);
    let mut first = true;
    for (i, it) in parsed.iter().enumerate() {
        let keep = !it.is_func || kept.contains(&i);
        if !keep {
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(&it.text);
    }
    out.push('\n');
    out.push_str(&module[outer_close..]);
    out
}

/// Parses the outer `(module …)`, returning `(byte offset of the first top-level item, the byte
/// ranges of every top-level list item, the byte offset of the outer closing paren)`.
fn parse_module(b: &[u8]) -> Option<(usize, Vec<(usize, usize)>, usize)> {
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

/// Extracts the `$name` of a `(func …)` item, or `None` for an anonymous function.
fn func_name(item: &str) -> Option<String> {
    let b = item.as_bytes();
    let mut i = skip_trivia(b, 1); // past '('
    i = skip_atom(b, i); // past `func`
    i = skip_trivia(b, i);
    if i < b.len() && b[i] == b'$' {
        let start = i;
        let end = skip_atom(b, i);
        Some(item[start..end].to_string())
    } else {
        None
    }
}

/// Every `$…` token in `s`, ignoring string literals and `;;` line comments. Block comments are not
/// specially handled: a stray token inside one can only cause a function to be *kept*, which is safe.
fn dollar_tokens(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut out = Vec::new();
    while i < n {
        match b[i] {
            b'"' => i = skip_string(b, i),
            b';' if i + 1 < n && b[i + 1] == b';' => {
                i += 2;
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'$' => {
                let start = i;
                let end = skip_atom(b, i);
                out.push(s[start..end].to_string());
                i = end;
            }
            _ => i += 1,
        }
    }
    out
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
        assert!(out.contains("export \"main\""), "exported shim kept:\n{}", out);
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
        assert!(out.contains("$a") && out.contains("$b"), "closure kept:\n{}", out);
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
}
