//! Auto-import quick fixes and completion edits for missing `import system…;` lines.

use dream_stdlib::symbol_to_package;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position as LspPosition, Range as LspRange,
    TextEdit, WorkspaceEdit,
};

use crate::conversions::map_position;
use crate::position::LineIndex;

/// Byte offset where a new `import …;` line should be inserted, and the LSP position for that edit.
pub fn import_insert_point(text: &str) -> (usize, LspPosition) {
    let line_index = LineIndex::new(text);
    let bytes = text.as_bytes();
    let mut last_import_end: Option<usize> = None;
    let mut module_end: Option<usize> = None;

    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] == b'\n' || bytes[i] == b'\r') {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if text[i..].starts_with("module ") {
            while i < bytes.len() && bytes[i] != b';' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
            module_end = Some(i);
            continue;
        }
        if text[i..].starts_with("import ") {
            while i < bytes.len() && bytes[i] != b';' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
            last_import_end = Some(i);
            continue;
        }
        break;
    }

    let offset = last_import_end.or(module_end).unwrap_or(0);
    (offset, map_position(line_index.position(offset)))
}

/// True when `text` already has `import <package>;`.
pub fn already_imports(text: &str, package: &str) -> bool {
    let needle = format!("import {};", package);
    text.lines().any(|l| l.trim() == needle)
}

/// Packages already imported via plain `import system…;`.
pub fn imported_packages(text: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("import ") {
            if let Some(path) = rest.strip_suffix(';') {
                if path.starts_with("system") && !path.contains(" as ") {
                    set.insert(path.trim().to_string());
                }
            }
        }
    }
    set
}

/// Workspace edit that inserts `import <package>;` at the top import block.
pub fn import_edit(uri: &tower_lsp::lsp_types::Url, text: &str, package: &str) -> Option<WorkspaceEdit> {
    if already_imports(text, package) {
        return None;
    }
    let (_, pos) = import_insert_point(text);
    let new_text = format!("import {};\n", package);
    let edit = TextEdit {
        range: LspRange::new(pos, pos),
        new_text,
    };
    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// Text edits (for completion `additionalTextEdits`) inserting an import.
pub fn import_text_edits(text: &str, package: &str) -> Option<Vec<TextEdit>> {
    if already_imports(text, package) {
        return None;
    }
    let (_, pos) = import_insert_point(text);
    Some(vec![TextEdit {
        range: LspRange::new(pos, pos),
        new_text: format!("import {};\n", package),
    }])
}

/// Pull a likely identifier from a diagnostic message / cursor word.
pub fn unresolved_name_from_message(message: &str) -> Option<String> {
    // "variable X does not exist at: ..."
    if let Some(rest) = message.strip_prefix("variable ") {
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    // "Struct 'X' not found"
    if let Some(start) = message.find('\'') {
        let rest = &message[start + 1..];
        if let Some(end) = rest.find('\'') {
            let name = &rest[..end];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    // "Function does not exist"
    None
}

/// Code actions offering to import the stdlib package that exports `name`.
pub fn auto_import_actions(
    uri: &tower_lsp::lsp_types::Url,
    text: &str,
    name: &str,
) -> Vec<CodeActionOrCommand> {
    let map = symbol_to_package();
    let Some(package) = map.get(name).copied() else {
        return Vec::new();
    };
    let Some(edit) = import_edit(uri, text, package) else {
        return Vec::new();
    };
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Import '{}'", package),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(edit),
        is_preferred: Some(true),
        ..Default::default()
    })]
}

/// Stdlib completion candidates not yet in scope (label, package, detail).
pub fn unloaded_stdlib_completions(text: &str) -> Vec<(String, &'static str, String)> {
    let imported = imported_packages(text);
    let map = symbol_to_package();
    let mut out = Vec::new();
    for (name, package) in map {
        if imported.contains(package) {
            continue;
        }
        out.push((
            name.clone(),
            package,
            format!("(import {})", package),
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
