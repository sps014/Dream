//! Read-only queries over the built [`Index`]: hover, go-to-definition, signature help,
//! completion, and the scope/name-resolution helpers they share.

use super::{is_ident_byte, keywords, Decl, Index, Located, Ref, SymKind, GLOBAL};
use dream::syntax::nodes::types::CONSTRUCTOR_NAME;

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
                    SymKind::Function | SymKind::Struct | SymKind::Enum | SymKind::Variable
                )
        })
    }

    /// Resolves any field or method named `name` (the first match across all structs), used as a
    /// fallback for member access where the precise receiver type is unknown.
    fn resolve_member(&self, name: &str) -> Option<&Decl> {
        self.decls.iter().find(|d| {
            d.name == name
                && matches!(
                    d.kind,
                    SymKind::Field | SymKind::Method | SymKind::EnumMember
                )
        })
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

    /// Extracts an immediately-preceding `receiver.` identifier (e.g. `Shape` in `Shape.Rect`),
    /// scanning back over an optional dot and surrounding spaces. Returns `None` when the reference
    /// is not a member access.
    fn receiver_before(text: &str, ref_start: usize) -> Option<&str> {
        let bytes = text.as_bytes();
        let mut i = ref_start;
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
            if recv_start < recv_end {
                return Some(&text[recv_start..recv_end]);
            }
        }
        None
    }

    pub(crate) fn substitute_generic(detail: &str, receiver_ty: &str) -> String {
        // `receiver_ty` is the human-readable type (e.g. `List<int>`); pull the generic argument
        // out of the angle brackets. Types are never `_`-mangled at this layer, so there is no
        // ambiguity with struct names that happen to contain underscores.
        let mut generic_arg = None;
        if let Some(start) = receiver_ty.find('<') {
            if let Some(end) = receiver_ty.rfind('>') {
                generic_arg = Some(&receiver_ty[start + 1..end]);
            }
        }

        let Some(generic_arg) = generic_arg else {
            return detail.to_string();
        };

        // This is a naive substitution (replaces whole words).
        detail
            .replace("<T>", &format!("<{}>", generic_arg))
            .replace(": T", &format!(": {}", generic_arg))
            .replace(" T,", &format!(" {},", generic_arg))
            .replace(" T)", &format!(" {})", generic_arg))
            .replace(" T>", &format!(" {}>", generic_arg))
            .replace(" T ", &format!(" {} ", generic_arg))
    }

    pub fn hover(&self, offset: usize, text: &str) -> Option<Located> {
        let mut receiver_ty_opt = None;
        let (start, end, decl) = if let Some(decl) = self.decl_at(offset) {
            (decl.start, decl.end, decl)
        } else {
            let reference = self.ref_at(offset)?;
            let receiver = Self::receiver_before(text, reference.start);
            let d = match reference.kind {
                SymKind::EnumMember => self.resolve_enum_member(receiver, &reference.name),
                SymKind::Field | SymKind::Method => {
                    // Try to infer the receiver's type so generic details (e.g. `List<int>`) can be
                    // substituted into the member signature below.
                    if let Some(recv) = receiver {
                        receiver_ty_opt =
                            self.variable_type(recv, reference.scope, reference.start);
                    }
                    self.resolve_member(&reference.name)
                }
                _ => self.resolve(&reference.name, reference.scope, reference.start),
            }?;
            (reference.start, reference.end, d)
        };

        let mut detail = decl.detail.clone();
        if let Some(receiver_ty) = receiver_ty_opt {
            detail = Self::substitute_generic(&detail, &receiver_ty);
        }

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
            SymKind::Field | SymKind::Method | SymKind::EnumMember => {
                self.resolve_member(&reference.name)
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
                        SymKind::Param | SymKind::Keyword | SymKind::Type => false,
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
                    && !matches!(d.kind, SymKind::Param | SymKind::Keyword | SymKind::Type)
                    && (needle.is_empty() || d.name.to_lowercase().contains(&needle))
            })
            .collect()
    }

    pub fn signature_help(&self, text: &str, offset: usize) -> Option<Decl> {
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
            let receiver_ty_opt = self.variable_type(receiver_obj, scope, recv_obj_start);

            if let Some(decl) = self.resolve_member(name) {
                let mut d = decl.clone();
                if let Some(receiver_ty) = receiver_ty_opt {
                    d.detail = Self::substitute_generic(&d.detail, &receiver_ty);
                }
                return Some(d);
            }
        } else {
            if let Some(decl) = self.resolve(name, scope, recv_start) {
                if decl.kind == SymKind::Struct {
                    if let Some(ctor_decl) = self.decls.iter().find(|d| {
                        d.name == CONSTRUCTOR_NAME
                            && d.kind == SymKind::Method
                            && d.detail.starts_with(&format!("{}.", name))
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
                    && d.detail.starts_with(&format!("{}.", name))
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

        // Check for import path completion
        let mut i = offset;
        while i > 0 && bytes[i - 1] != b'"' && bytes[i - 1] != b'\n' {
            i -= 1;
        }
        if i > 0 && bytes[i - 1] == b'"' {
            let mut j = i - 1;
            while j > 0 && (bytes[j - 1] == b' ' || bytes[j - 1] == b'\t') {
                j -= 1;
            }
            if j >= 6 && &text[j - 6..j] == "import" {
                let mut out = Vec::new();
                if let Some(path_str) = file_path {
                    let parent_dir = std::path::Path::new(path_str)
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new(""));
                    let current_dir = if offset > i {
                        parent_dir.join(&text[i..offset])
                    } else {
                        parent_dir.to_path_buf()
                    };

                    let search_dir = if current_dir.is_dir() {
                        current_dir.clone()
                    } else {
                        current_dir
                            .parent()
                            .unwrap_or_else(|| std::path::Path::new(""))
                            .to_path_buf()
                    };

                    if let Ok(entries) = std::fs::read_dir(&search_dir) {
                        for entry in entries.flatten() {
                            if let Ok(file_type) = entry.file_type() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if file_type.is_dir() {
                                    out.push((
                                        name,
                                        SymKind::Variable,
                                        "directory".to_string(),
                                        None,
                                    ));
                                } else if name.ends_with(".dream") {
                                    out.push((name, SymKind::Variable, "module".to_string(), None));
                                }
                            }
                        }
                    }
                }
                return out;
            }
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
                SymKind::Function | SymKind::Struct | SymKind::Enum => {
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

    /// Members available on `receiver`, resolved by type. If `receiver` is a variable/parameter
    /// (including `this`) whose type is a known struct, only that struct's fields and methods are
    /// offered. If `receiver` names an enum, its members are offered. Otherwise nothing is
    /// offered, so member access never dumps unrelated symbols.
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
            return self.members_of_struct(base);
        }

        // A bare struct name used as a receiver (e.g. static method access `Point.`).
        if self
            .decls
            .iter()
            .any(|d| d.kind == SymKind::Struct && d.name == receiver)
        {
            return self.members_of_struct(receiver);
        }

        Vec::new()
    }

    fn members_of_struct(&self, ty: &str) -> Vec<(String, SymKind, String, Option<String>)> {
        // `ty` may carry generic arguments (`Box<int>`); members are registered under the bare
        // struct name (`Box.value`), so match on that while keeping the full type for argument
        // substitution in member signatures.
        let base = ty.split('<').next().unwrap_or(ty).trim();
        let prefix = format!("{}.", base);
        self.decls
            .iter()
            .filter(|d| {
                matches!(d.kind, SymKind::Field | SymKind::Method)
                    && d.scope == GLOBAL
                    && d.detail.starts_with(&prefix)
                    && d.name != CONSTRUCTOR_NAME
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
