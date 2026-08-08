//! Read-only queries over the built [`Index`]: hover, go-to-definition, signature help,
//! completion, and the scope/name-resolution helpers they share.

use super::attr_ide::{
    attribute_arg_completions, attribute_arg_context, attribute_hover, attribute_name_completions,
    attribute_name_partial, attribute_signature,
};
use super::{
    is_ident_byte, keywords, substitute_method_type_args, substitute_type_param_t, type_base, Decl,
    Index, Located, Ref, SymKind, GLOBAL,
};
use super::detail_is_static_method;
use super::detail_belongs_to;
use crate::code_actions::imported_packages;
use dream::syntax::nodes::types::CONSTRUCTOR_NAME;
use dream_abi::attributes::find_spec;
use dream_stdlib::{BOOTSTRAP_PACKAGES, STD_PACKAGES};

/// Walks upward from `start_dir` looking for a `dream_packages/` directory (installed by the
/// `dreamer` package manager), stopping without a match at the first `dream.toml` project root
/// that has none yet. Mirrors `find_dream_packages_dir` in `src/driver/source_loader.rs` — kept
/// as a separate copy since it's a few lines of pure filesystem-walking with no shared state, and
/// duplicating it avoids making the LSP depend on a compiler-internal, non-`pub` helper.
fn find_dream_packages_dir(start_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = Some(start_dir.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join("dream_packages");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if d.join("dream.toml").is_file() {
            return None;
        }
        dir = d.parent().map(std::path::Path::to_path_buf);
    }
    None
}

/// True when `offset` is in a `receiver.` / `receiver.partial` member-access position.
/// Used by the LSP backend to avoid merging unloaded stdlib type completions into
/// member lists (`System.` must not offer `List` / `Gpu` / …).
pub fn is_member_completion_context(text: &str, offset: usize) -> bool {
    if import_path_partial(text, offset).is_some() {
        return false;
    }
    let bytes = text.as_bytes();
    let offset = offset.min(bytes.len());
    let mut i = offset;
    while i > 0 && is_ident_byte(bytes[i - 1]) {
        i -= 1;
    }
    i > 0 && bytes[i - 1] == b'.'
}

/// If `offset` is inside an unquoted `import <path>` statement, returns
/// `(path_start_byte, partial_path)` where `partial_path` is the text from the path start to
/// the cursor (e.g. `""`, `system`, `system.`). Outside import context returns `None` so
/// `System.` member completion is unaffected.
pub(crate) fn import_path_partial(text: &str, offset: usize) -> Option<(usize, String)> {
    let bytes = text.as_bytes();
    let offset = offset.min(bytes.len());

    // Walk back over the partial dotted path.
    let mut path_start = offset;
    while path_start > 0 {
        let b = bytes[path_start - 1];
        if is_ident_byte(b) || b == b'.' {
            path_start -= 1;
        } else {
            break;
        }
    }

    // Skip whitespace between `import` and the path.
    let mut j = path_start;
    while j > 0 && (bytes[j - 1] == b' ' || bytes[j - 1] == b'\t') {
        j -= 1;
    }

    if j < 6 {
        return None;
    }
    if &text[j - 6..j] != "import" {
        return None;
    }
    // Word boundary before `import`.
    if j > 6 {
        let before = bytes[j - 7];
        if is_ident_byte(before) {
            return None;
        }
    }
    // Must be on the same line as `import` (no newline between import and cursor).
    if text[j..offset].contains('\n') {
        return None;
    }

    Some((path_start, text[path_start..offset].to_string()))
}

fn push_module_completion(
    out: &mut Vec<(String, SymKind, String, Option<String>)>,
    name: String,
    detail: &str,
    imported: &std::collections::HashSet<String>,
) {
    if imported.contains(&name) {
        return;
    }
    if out.iter().any(|(n, ..)| n == &name) {
        return;
    }
    out.push((
        name,
        SymKind::Module,
        detail.to_string(),
        None,
    ));
}

/// Package / local-module completions for an unquoted `import` path prefix.
fn import_path_completions(
    file_path: Option<&str>,
    text: &str,
    partial: &str,
) -> Vec<(String, SymKind, String, Option<String>)> {
    let imported = imported_packages(text);
    let mut out = Vec::new();

    for pkg in STD_PACKAGES {
        if BOOTSTRAP_PACKAGES.contains(&pkg.name) {
            continue;
        }
        if pkg.name.starts_with(partial) {
            push_module_completion(&mut out, pkg.name.to_string(), "stdlib package", &imported);
        }
    }

    if let Some(path_str) = file_path {
        let parent_dir = std::path::Path::new(path_str)
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""));

        // Local `.dream` files / directories as dotted relative modules.
        if let Ok(entries) = std::fs::read_dir(parent_dir) {
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().to_string();
                if file_type.is_dir() {
                    if name.starts_with('.') || name == "dream_packages" {
                        continue;
                    }
                    if name.starts_with(partial) || format!("{}.", name).starts_with(partial) {
                        push_module_completion(&mut out, name, "directory", &imported);
                    }
                } else if let Some(stem) = name.strip_suffix(".dream") {
                    if stem.starts_with(partial) {
                        push_module_completion(
                            &mut out,
                            stem.to_string(),
                            "module",
                            &imported,
                        );
                    }
                }
            }
        }

        if let Some(packages_dir) = find_dream_packages_dir(parent_dir) {
            if let Ok(entries) = std::fs::read_dir(&packages_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with(partial) {
                            push_module_completion(&mut out, name, "package", &imported);
                        }
                    }
                }
            }
        }
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

impl Index {
    fn span_at(start: usize, end: usize, offset: usize) -> bool {
        offset >= start && offset <= end
    }

    /// Returns the declaration whose name token is under `offset`, if any.
    fn decl_at(&self, offset: usize) -> Option<&Decl> {
        self.decls
            .iter()
            .find(|d| d.is_main && Self::span_at(d.start, d.end, offset))
    }

    /// Returns the reference whose name token is under `offset`, if any.
    fn ref_at(&self, offset: usize) -> Option<&Ref> {
        self.refs
            .iter()
            .find(|r| r.is_main && Self::span_at(r.start, r.end, offset))
    }

    /// Resolves a name used at `offset` within `scope` to its declaration. Locals (variables and
    /// parameters declared at or before the use site, in the same function) take precedence over
    /// file-scope declarations, approximating lexical scoping without block-level precision.
    fn resolve(&self, name: &str, scope: usize, before: usize) -> Option<&Decl> {
        let local = self
            .decls
            .iter()
            .filter(|d| {
                d.name == name
                    && d.scope == scope
                    && matches!(d.kind, SymKind::Variable | SymKind::Param)
                    && d.start <= before
            })
            .max_by_key(|d| d.start);
        if local.is_some() {
            return local;
        }
        // File-scope fallback: free functions, types, and top-level `let`/`const` globals (which
        // carry `scope == GLOBAL` and `SymKind::Variable`).
        self.decls.iter().find(|d| {
            d.name == name
                && d.scope == GLOBAL
                && matches!(
                    d.kind,
                    SymKind::Function
                        | SymKind::Class
                        | SymKind::Struct
                        | SymKind::Interface
                        | SymKind::Enum
                        | SymKind::Variable
                )
        })
    }

    /// Resolves a field or method named `name`. When `receiver_ty` is known, prefer the member
    /// whose `detail` is qualified by that type (`Owner.` / `static Owner.` / …) so same-named
    /// members on different types (e.g. `js.global` vs `Regex.global`) disambiguate. A known
    /// receiver that has no matching member returns `None` — never a random same-named field.
    fn resolve_member(&self, receiver_ty: Option<&str>, name: &str) -> Option<&Decl> {
        if let Some(ty) = receiver_ty {
            let base = type_base(ty);
            return self.decls.iter().find(|d| {
                d.name == name
                    && matches!(d.kind, SymKind::Field | SymKind::Method)
                    && detail_belongs_to(&d.detail, base)
            });
        }
        self.decls.iter().find(|d| {
            d.name == name
                && matches!(
                    d.kind,
                    SymKind::Field | SymKind::Method | SymKind::EnumMember
                )
        })
    }

    /// Type of a receiver identifier: variable/param type, or the bare type name itself when it
    /// names a class/struct/interface/enum/extend-target (static access `ComputePass.dispatch` /
    /// `js.global`).
    fn receiver_type_name(&self, receiver: &str, scope: usize, before: usize) -> Option<String> {
        if let Some(ty) = self.variable_type(receiver, scope, before) {
            return Some(ty);
        }
        if self.decls.iter().any(|d| {
            d.name == receiver
                && matches!(
                    d.kind,
                    SymKind::Class
                        | SymKind::Struct
                        | SymKind::Interface
                        | SymKind::Enum
                        | SymKind::Type
                )
        }) {
            return Some(receiver.to_string());
        }
        None
    }

    /// Resolves an enum variant reference. When the receiver (the `Enum` in `Enum.Variant`) is
    /// known, prefer the variant whose `detail` is qualified by that enum so look-alike variant
    /// names across different enums (e.g. `Some`/`None`) disambiguate; otherwise fall back to the
    /// first variant matching by name.
    fn resolve_enum_member(&self, receiver: Option<&str>, name: &str) -> Option<&Decl> {
        if let Some(recv) = receiver {
            let prefix = format!("{}.", recv);
            if let Some(d) = self.decls.iter().find(|d| {
                d.kind == SymKind::EnumMember && d.name == name && d.detail.starts_with(&prefix)
            }) {
                return Some(d);
            }
        }
        self.decls
            .iter()
            .find(|d| d.kind == SymKind::EnumMember && d.name == name)
    }

    pub(crate) fn substitute_generic(detail: &str, receiver_ty: &str) -> String {
        // `receiver_ty` is the human-readable type (e.g. `List<int>` / `GpuBuffer<float>`);
        // pull the generic argument out of the angle brackets.
        let mut generic_arg = None;
        if let Some(start) = receiver_ty.find('<') {
            if let Some(end) = receiver_ty.rfind('>') {
                generic_arg = Some(&receiver_ty[start + 1..end]);
            }
        }

        let Some(generic_arg) = generic_arg else {
            return detail.to_string();
        };

        substitute_type_param_t(detail, generic_arg)
    }

    /// Applies call-site / receiver type arguments to a method detail that still mentions `T`
    /// (e.g. `GpuBuffer.alloc<float>` → `GpuBuffer<float>`, `read_at(): T[]` → `float[]`).
    pub(crate) fn apply_type_args_to_detail(
        detail: &str,
        receiver_ty: Option<&str>,
        call_type_args: &[String],
    ) -> String {
        let mut out = detail.to_string();
        // Prefer an already-concrete receiver (`GpuBuffer<float>`).
        if let Some(recv) = receiver_ty {
            if recv.contains('<') {
                out = Self::substitute_generic(&out, recv);
            } else if !call_type_args.is_empty() {
                // Bare `GpuBuffer.alloc<float>(…)`: synthesize `GpuBuffer<float>`.
                let synthetic = format!("{}<{}>", type_base(recv), call_type_args.join(", "));
                out = Self::substitute_generic(&out, &synthetic);
            }
        } else if call_type_args.len() == 1 {
            out = substitute_type_param_t(&out, &call_type_args[0]);
        }
        if !call_type_args.is_empty() {
            out = substitute_method_type_args(&out, call_type_args);
        }
        out
    }

    pub fn hover(&self, text: &str, offset: usize) -> Option<Located> {
        let mut receiver_ty_opt = None;
        let (start, end, decl) = if let Some(decl) = self.decl_at(offset) {
            (decl.start, decl.end, decl)
        } else {
            let reference = self.ref_at(offset)?;
            if reference.kind == SymKind::Decorator {
                let spec = find_spec(&reference.name)?;
                return Some(Located {
                    start: reference.start,
                    end: reference.end,
                    contents: attribute_hover(spec),
                });
            }
            let receiver = reference.receiver.as_deref();
            let d = match reference.kind {
                SymKind::EnumMember => self.resolve_enum_member(receiver, &reference.name),
                SymKind::Field | SymKind::Method => {
                    let mut recv_ty = None;
                    if let Some(recv) = receiver {
                        recv_ty =
                            self.receiver_type_name(recv, reference.scope, reference.start);
                        receiver_ty_opt = recv_ty.clone();
                    }
                    self.resolve_member(recv_ty.as_deref(), &reference.name)
                }
                _ => self.resolve(&reference.name, reference.scope, reference.start),
            }?;
            (reference.start, reference.end, d)
        };

        let mut type_args = if decl.kind == SymKind::Method {
            method_type_args_at(text, end).unwrap_or_default()
        } else {
            Vec::new()
        };
        // `GpuBuffer<float>.alloc` puts class args before the `.`, not after the method name.
        if type_args.is_empty() && decl.kind == SymKind::Method {
            if let Some(args) = type_args_before_member_dot(text, start) {
                type_args = args;
            }
        }
        let detail = Self::apply_type_args_to_detail(
            &decl.detail,
            receiver_ty_opt.as_deref(),
            &type_args,
        );

        let mut contents = format!("```dream\n{}\n```", detail);
        if let Some(doc) = &decl.doc_comment {
            contents.push_str("\n\n---\n\n");
            contents.push_str(doc);
        }

        Some(Located {
            start,
            end,
            contents,
        })
    }

    /// Resolves the declaration the cursor sits on, whether `offset` lands on the declaration's
    /// own name or on a reference to it. Shared by go-to-definition and find-references.
    fn decl_for_offset(&self, offset: usize) -> Option<&Decl> {
        if let Some(decl) = self.decl_at(offset) {
            return Some(decl);
        }
        let reference = self.ref_at(offset)?;
        match reference.kind {
            SymKind::EnumMember => {
                self.resolve_enum_member(reference.receiver.as_deref(), &reference.name)
            }
            SymKind::Field | SymKind::Method => {
                let recv_ty = reference.receiver.as_ref().and_then(|recv| {
                    self.receiver_type_name(recv, reference.scope, reference.start)
                });
                self.resolve_member(recv_ty.as_deref(), &reference.name)
            }
            _ => self.resolve(&reference.name, reference.scope, reference.start),
        }
    }

    pub fn definition(&self, offset: usize) -> Option<(usize, usize)> {
        self.decl_for_offset(offset).map(|d| (d.start, d.end))
    }

    /// All occurrences (byte spans) of the symbol under `offset`: the declaration (when
    /// `include_declaration`) plus every recorded reference that resolves to it. Locals and
    /// parameters are confined to their function scope; everything else matches by name across the
    /// document, mirroring the index's best-effort resolution.
    pub fn references(&self, offset: usize, include_declaration: bool) -> Vec<(usize, usize)> {
        let Some(decl) = self.decl_for_offset(offset) else {
            return Vec::new();
        };
        let name = decl.name.clone();
        let is_local =
            matches!(decl.kind, SymKind::Param | SymKind::Variable) && decl.scope != GLOBAL;
        let scope = decl.scope;
        let decl_span = (decl.start, decl.end);

        let mut out = Vec::new();
        if include_declaration {
            out.push(decl_span);
        }
        for r in &self.refs {
            if !r.is_main || r.name != name {
                continue;
            }
            if is_local && r.scope != scope {
                continue;
            }
            out.push((r.start, r.end));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The document's outline: top-level declarations (functions, types, enum members, fields,
    /// methods, and file-scope globals), excluding locals and parameters. Used for the document
    /// symbols / outline view.
    pub fn document_symbols(&self) -> Vec<&Decl> {
        self.decls
            .iter()
            .filter(|d| {
                d.is_main
                    && match d.kind {
                        SymKind::Variable => d.scope == GLOBAL,
                        SymKind::Param | SymKind::Keyword | SymKind::Type | SymKind::Decorator => {
                            false
                        }
                        _ => true,
                    }
            })
            .collect()
    }

    /// Workspace "go to symbol" candidates whose name matches `query` (case-insensitive substring;
    /// an empty query matches every candidate). Uses the same named-declaration filter as
    /// [`document_symbols`](Self::document_symbols) but drops the `scope == GLOBAL` restriction on
    /// variables so any named declaration is discoverable, mirroring how editors surface symbols
    /// across a workspace.
    pub fn symbols_matching(&self, query: &str) -> Vec<&Decl> {
        let needle = query.to_lowercase();
        self.decls
            .iter()
            .filter(|d| {
                d.is_main
                    && !matches!(
                        d.kind,
                        SymKind::Param | SymKind::Keyword | SymKind::Type | SymKind::Decorator
                    )
                    && (needle.is_empty() || d.name.to_lowercase().contains(&needle))
            })
            .collect()
    }

    pub fn signature_help(&self, text: &str, offset: usize) -> Option<Decl> {
        if let Some((spec, _active)) = attribute_signature(text, offset) {
            return Some(Decl {
                name: spec.name.to_string(),
                kind: SymKind::Decorator,
                detail: spec.args.signature_label(spec.name),
                doc_comment: Some(spec.doc.to_string()),
                start: 0,
                end: 0,
                scope: GLOBAL,
                ty: None,
                is_main: true,
            });
        }

        let bytes = text.as_bytes();
        let mut i = offset;
        let mut paren_count = 0;
        let mut open_paren_offset = None;

        while i > 0 {
            i -= 1;
            let b = bytes[i];
            if b == b')' {
                paren_count += 1;
            } else if b == b'(' {
                if paren_count > 0 {
                    paren_count -= 1;
                } else {
                    open_paren_offset = Some(i);
                    break;
                }
            } else if b == b';' || b == b'{' || b == b'}' {
                return None;
            }
        }

        let op_idx = open_paren_offset?;
        let mut j = op_idx;
        while j > 0 && (bytes[j - 1] == b' ' || bytes[j - 1] == b'\t' || bytes[j - 1] == b'\n') {
            j -= 1;
        }
        let recv_end = j;
        let mut recv_start = recv_end;
        while recv_start > 0 && is_ident_byte(bytes[recv_start - 1]) {
            recv_start -= 1;
        }

        if recv_start == recv_end {
            return None;
        }

        let name = &text[recv_start..recv_end];
        let scope = self.enclosing_scope(offset);

        let mut k = recv_start;
        while k > 0 && (bytes[k - 1] == b' ' || bytes[k - 1] == b'\t' || bytes[k - 1] == b'\n') {
            k -= 1;
        }
        if k > 0 && bytes[k - 1] == b'.' {
            let mut j2 = k - 1;
            while j2 > 0 && bytes[j2 - 1] == b' ' {
                j2 -= 1;
            }
            let recv_obj_end = j2;
            let mut recv_obj_start = recv_obj_end;
            while recv_obj_start > 0 && is_ident_byte(bytes[recv_obj_start - 1]) {
                recv_obj_start -= 1;
            }
            let receiver_obj = &text[recv_obj_start..recv_obj_end];
            let receiver_ty_opt =
                self.receiver_type_name(receiver_obj, scope, recv_obj_start);

            if let Some(decl) = self.resolve_member(receiver_ty_opt.as_deref(), name) {
                let mut d = decl.clone();
                let mut type_args = method_type_args_at(text, recv_end).unwrap_or_default();
                if type_args.is_empty() {
                    if let Some(args) = type_args_before_member_dot(text, recv_start) {
                        type_args = args;
                    }
                }
                d.detail = Self::apply_type_args_to_detail(
                    &d.detail,
                    receiver_ty_opt.as_deref(),
                    &type_args,
                );
                return Some(d);
            }
        } else {
            if let Some(decl) = self.resolve(name, scope, recv_start) {
                if matches!(decl.kind, SymKind::Class | SymKind::Struct) {
                    if let Some(ctor_decl) = self.decls.iter().find(|d| {
                        d.name == CONSTRUCTOR_NAME
                            && d.kind == SymKind::Method
                            && detail_belongs_to(&d.detail, name)
                    }) {
                        return Some(ctor_decl.clone());
                    }
                } else {
                    return Some(decl.clone());
                }
            }
            // For struct initializers where `resolve` failed entirely (e.g. static imports sometimes)
            if let Some(decl) = self.decls.iter().find(|d| {
                d.name == CONSTRUCTOR_NAME
                    && d.kind == SymKind::Method
                    && detail_belongs_to(&d.detail, name)
            }) {
                return Some(decl.clone());
            }
        }

        None
    }

    /// Type name of a variable/parameter named `name` visible at `before` within `scope`.
    fn variable_type(&self, name: &str, scope: usize, before: usize) -> Option<String> {
        self.resolve(name, scope, before).and_then(|d| d.ty.clone())
    }

    /// Completion proposals at `offset`. After a `.` we attempt member completion against the
    /// receiver's resolved struct type, falling back to all members when the type is unknown.
    pub fn completions(
        &self,
        file_path: Option<&str>,
        text: &str,
        offset: usize,
    ) -> Vec<(String, SymKind, String, Option<String>)> {
        let scope = self.enclosing_scope(offset);
        let bytes = text.as_bytes();

        // Unquoted `import system…` package paths — must run before member `.` so
        // `import system.` is not treated as a variable member access. Expression
        // `System.` still falls through to member_completions below.
        if let Some((_path_start, partial)) = import_path_partial(text, offset) {
            return import_path_completions(file_path, text, &partial);
        }

        // `@name` / `@partial` attribute-name completion (before any `.` / keyword dump).
        if let Some((_name_start, partial)) = attribute_name_partial(text, offset) {
            return attribute_name_completions(&partial)
                .into_iter()
                .map(|(label, _insert, detail, doc)| (label, SymKind::Decorator, detail, doc))
                .collect();
        }

        // Inside `@name(...)` — closed-world arg suggestions when the registry knows them.
        // Always stay in attribute-arg mode (never dump keywords / globals into the arg list).
        if let Some(ctx) = attribute_arg_context(text, offset) {
            return attribute_arg_completions(&ctx)
                .into_iter()
                .map(|(label, _insert, detail, doc)| (label, SymKind::Decorator, detail, doc))
                .collect();
        }

        // Detect `receiver.<partial>` by scanning back over an identifier and a dot.
        let mut i = offset;
        while i > 0 && is_ident_byte(bytes[i - 1]) {
            i -= 1;
        }
        if i > 0 && bytes[i - 1] == b'.' {
            let mut j = i - 1;
            while j > 0 && bytes[j - 1] == b' ' {
                j -= 1;
            }
            let recv_end = j;
            let mut recv_start = recv_end;
            while recv_start > 0 && is_ident_byte(bytes[recv_start - 1]) {
                recv_start -= 1;
            }
            let receiver = &text[recv_start..recv_end];

            // `Type.staticMember.` — e.g. `js.global.` — complete against the static
            // member's return type (instance helpers on `js`), not the bare identifier.
            if recv_start > 0 && bytes[recv_start - 1] == b'.' {
                let mut k = recv_start - 1;
                while k > 0 && bytes[k - 1] == b' ' {
                    k -= 1;
                }
                let outer_end = k;
                let mut outer_start = outer_end;
                while outer_start > 0 && is_ident_byte(bytes[outer_start - 1]) {
                    outer_start -= 1;
                }
                let outer = &text[outer_start..outer_end];
                if let Some(ret_ty) = self.static_member_return_type(outer, receiver) {
                    return self.members_of_struct(&ret_ty, /*static_only*/ false);
                }
            }

            return self.member_completions(receiver, scope, recv_start);
        }

        let mut out = Vec::new();
        for kw in keywords() {
            out.push((
                kw.to_string(),
                SymKind::Keyword,
                "keyword".to_string(),
                None,
            ));
        }
        for d in &self.decls {
            match d.kind {
                SymKind::Function
                | SymKind::Class
                | SymKind::Struct
                | SymKind::Interface
                | SymKind::Enum
                | SymKind::Type => {
                    out.push((
                        d.name.clone(),
                        d.kind,
                        d.detail.clone(),
                        d.doc_comment.clone(),
                    ));
                }
                // Top-level `let`/`const` globals are visible from every body.
                SymKind::Variable if d.scope == GLOBAL => {
                    out.push((
                        d.name.clone(),
                        d.kind,
                        d.detail.clone(),
                        d.doc_comment.clone(),
                    ));
                }
                SymKind::Variable | SymKind::Param if d.scope == scope && d.start <= offset => {
                    out.push((
                        d.name.clone(),
                        d.kind,
                        d.detail.clone(),
                        d.doc_comment.clone(),
                    ));
                }
                _ => {}
            }
        }
        out
    }

    /// Return type of a static method `Type.member` when both are indexed (e.g. `js.global` → `js`).
    fn static_member_return_type(&self, type_name: &str, member: &str) -> Option<String> {
        let is_type = self.decls.iter().any(|d| {
            d.name == type_name
                && matches!(
                    d.kind,
                    SymKind::Class
                        | SymKind::Struct
                        | SymKind::Interface
                        | SymKind::Enum
                        | SymKind::Type
                )
        });
        if !is_type {
            return None;
        }
        let detail = self
            .decls
            .iter()
            .find(|d| {
                d.kind == SymKind::Method
                    && d.name == member
                    && detail_is_static_method(&d.detail)
                    && detail_belongs_to(&d.detail, type_name)
            })
            .map(|d| d.detail.as_str())?;
        detail
            .rfind(':')
            .map(|i| detail[i + 1..].trim().to_string())
    }

    /// Members available on `receiver`, resolved by type. If `receiver` is a variable/parameter
    /// (including `this`) whose type is a known struct, only that struct's fields and methods are
    /// offered. If `receiver` names an enum, its members are offered. A bare class/struct name
    /// only offers **static** methods (instance methods require a value receiver).
    fn member_completions(
        &self,
        receiver: &str,
        scope: usize,
        before: usize,
    ) -> Vec<(String, SymKind, String, Option<String>)> {
        // `Type.` / `Color.` static or enum access: the receiver itself names a struct or enum.
        if self
            .decls
            .iter()
            .any(|d| d.kind == SymKind::Enum && d.name == receiver)
        {
            return self.members_of_enum(receiver);
        }

        if let Some(ty) = self.variable_type(receiver, scope, before) {
            let base = ty.trim_end_matches('?').trim_end_matches("[]");
            return self.members_of_struct(base, /*static_only*/ false);
        }

        // A bare class/struct/interface/type name used as a receiver (e.g. static `Point.` / `js.`).
        if self.decls.iter().any(|d| {
            matches!(
                d.kind,
                SymKind::Class | SymKind::Struct | SymKind::Interface | SymKind::Type
            ) && d.name == receiver
        }) {
            return self.members_of_struct(receiver, /*static_only*/ true);
        }

        Vec::new()
    }

    fn members_of_struct(
        &self,
        ty: &str,
        static_only: bool,
    ) -> Vec<(String, SymKind, String, Option<String>)> {
        // `ty` may carry generic arguments (`Box<int>`); members are registered under the bare
        // struct name (`Box.value`), so match on that while keeping the full type for argument
        // substitution in member signatures.
        let base = type_base(ty);
        self.decls
            .iter()
            .filter(|d| {
                matches!(d.kind, SymKind::Field | SymKind::Method)
                    && d.scope == GLOBAL
                    && detail_belongs_to(&d.detail, base)
                    && d.name != CONSTRUCTOR_NAME
                    && if static_only {
                        // Type-name access: only static methods (no fields / instance methods).
                        d.kind == SymKind::Method && detail_is_static_method(&d.detail)
                    } else {
                        // Value receiver: fields + instance methods (not static).
                        d.kind == SymKind::Field
                            || (d.kind == SymKind::Method && !detail_is_static_method(&d.detail))
                    }
            })
            .map(|d| {
                let detail = Self::substitute_generic(&d.detail, ty);
                (d.name.clone(), d.kind, detail, d.doc_comment.clone())
            })
            .collect()
    }

    fn members_of_enum(&self, name: &str) -> Vec<(String, SymKind, String, Option<String>)> {
        let prefix = format!("{}.", name);
        self.decls
            .iter()
            .filter(|d| d.kind == SymKind::EnumMember && d.detail.starts_with(&prefix))
            .map(|d| {
                (
                    d.name.clone(),
                    d.kind,
                    d.detail.clone(),
                    d.doc_comment.clone(),
                )
            })
            .collect()
    }

    /// The function scope whose body span contains `offset`, or [`GLOBAL`].
    fn enclosing_scope(&self, offset: usize) -> usize {
        // Parameters/locals of a function share its scope id and are appended in source order,
        // so the latest local/param declared before `offset` identifies the enclosing function.
        let mut best: Option<(usize, usize)> = None; // (scope, name_start)
        for d in &self.decls {
            if matches!(d.kind, SymKind::Param | SymKind::Variable)
                && d.scope != GLOBAL
                && d.start <= offset
            {
                match best {
                    Some((_, s)) if s >= d.start => {}
                    _ => best = Some((d.scope, d.start)),
                }
            }
        }
        best.map(|(scope, _)| scope).unwrap_or(GLOBAL)
    }
}

/// Parses call-site method type arguments after a method name token: `dispatch<int, string>(`.
/// `name_end` is the exclusive end offset of the method identifier.
fn method_type_args_at(text: &str, name_end: usize) -> Option<Vec<String>> {
    let bytes = text.as_bytes();
    let mut i = name_end.min(bytes.len());
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'<' {
        return None;
    }
    let start = i + 1;
    let mut depth = 1i32;
    let mut j = start;
    while j < bytes.len() {
        match bytes[j] {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    let inner = text[start..j].trim();
                    if inner.is_empty() {
                        return Some(Vec::new());
                    }
                    return Some(
                        inner
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect(),
                    );
                }
            }
            b'(' if depth == 1 => return None, // malformed
            _ => {}
        }
        j += 1;
    }
    None
}

/// Class type args in `GpuBuffer<float>.alloc` — the `<…>` sits before the `.`, not after the method.
fn type_args_before_member_dot(text: &str, member_start: usize) -> Option<Vec<String>> {
    let bytes = text.as_bytes();
    let mut i = member_start.min(bytes.len());
    while i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
        i -= 1;
    }
    if i == 0 || bytes[i - 1] != b'.' {
        return None;
    }
    i -= 1;
    while i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
        i -= 1;
    }
    if i == 0 || bytes[i - 1] != b'>' {
        return None;
    }
    let gt = i - 1;
    let mut depth = 1i32;
    let mut j = gt;
    while j > 0 {
        j -= 1;
        match bytes[j] {
            b'>' => depth += 1,
            b'<' => {
                depth -= 1;
                if depth == 0 {
                    let inner = text[j + 1..gt].trim();
                    if inner.is_empty() {
                        return Some(Vec::new());
                    }
                    return Some(
                        inner
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect(),
                    );
                }
            }
            _ => {}
        }
    }
    None
}
