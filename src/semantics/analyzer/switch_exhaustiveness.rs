//! Pattern type-checking and exhaustiveness for `switch`. Split out of `switch_unions.rs` (which
//! retains union construction, `switch`/`foreach` analysis, and HIR lowering): type-checks each
//! pattern against the subject type and introduces bindings (`check_pattern`), validates variant
//! payload arity/types, and checks exhaustiveness / redundant arms. Methods on `Analyzer`, in a
//! sibling module sharing its `pub(super)` surface.

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::semantics::errors::SemanticError;
use crate::semantics::symbol_table::SymbolTable;
use crate::semantics::union_table::UnionInfo;
use crate::syntax::nodes::types::strip_nullable;
use crate::syntax::nodes::{PatternNode, SwitchArm, Type};
use std::cell::RefCell;
use std::rc::Rc;

use super::switch_unions::PatternInfo;

impl<'a> Analyzer<'a> {
    /// Type-checks `pattern` against `expected`, introducing any bindings into `scope`.
    pub(super) fn check_pattern(
        &mut self,
        pattern: &PatternNode,
        expected: &Type,
        scope: &Rc<RefCell<SymbolTable>>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<PatternInfo, SemanticError> {
        let expected_base = strip_nullable(&expected.get_type()).to_string();
        let union_info: Option<UnionInfo> = self.union_table.get(&expected_base).cloned();

        match pattern {
            PatternNode::Wildcard(_) => Ok(PatternInfo { irrefutable: true }),
            PatternNode::Binding(name) => {
                // A bare identifier that names a unit variant of the matched union is a
                // unit-variant pattern; otherwise it binds the whole value.
                if let Some(info) = &union_info {
                    if let Some(v) = info.variant(&name.text) {
                        if v.fields.is_empty() {
                            return Ok(PatternInfo { irrefutable: false });
                        }
                    }
                }
                if let Err(e) = (*scope)
                    .borrow_mut()
                    .add_symbol(name.text.clone(), expected.clone())
                {
                    diagnostics.report_error(e.to_string(), Some(name.position));
                }
                Ok(PatternInfo { irrefutable: true })
            }
            PatternNode::Literal(lit) => {
                if !lit.is_unknown()
                    && !expected.is_unknown()
                    && !self.type_str_assignable(&expected_base, &lit.get_type())
                {
                    diagnostics.report_error(
                        format!(
                            "Pattern literal of type '{}' cannot match a value of type '{}'",
                            lit.get_type(),
                            expected_base
                        ),
                        lit.get_span(),
                    );
                }
                Ok(PatternInfo { irrefutable: false })
            }
            PatternNode::Variant(qualifier, variant, subs) => {
                let info = match &union_info {
                    Some(info) => info.clone(),
                    None => {
                        diagnostics.report_error(
                            format!(
                                "Variant pattern '{}' can only match a discriminated union, not '{}'",
                                variant.text, expected_base
                            ),
                            Some(variant.position),
                        );
                        // Still walk sub-patterns so their bindings/errors surface.
                        for sub in subs {
                            self.check_pattern(sub, &Type::Unknown, scope, diagnostics)?;
                        }
                        return Ok(PatternInfo { irrefutable: false });
                    }
                };

                if let Some(q) = qualifier {
                    if q.text != expected_base {
                        diagnostics.report_error(
                            format!(
                                "Variant qualifier '{}' does not match the matched enum '{}'",
                                q.text, expected_base
                            ),
                            Some(q.position),
                        );
                    }
                }

                let var_info = match info.variant(&variant.text) {
                    Some(v) => v.clone(),
                    None => {
                        diagnostics.report_error(
                            format!("Enum '{}' has no variant '{}'", expected_base, variant.text),
                            Some(variant.position),
                        );
                        for sub in subs {
                            self.check_pattern(sub, &Type::Unknown, scope, diagnostics)?;
                        }
                        return Ok(PatternInfo { irrefutable: false });
                    }
                };

                if subs.len() != var_info.fields.len() {
                    diagnostics.report_error(
                        format!(
                            "Variant '{}.{}' has {} field(s), but the pattern binds {}",
                            expected_base,
                            variant.text,
                            var_info.fields.len(),
                            subs.len()
                        ),
                        Some(variant.position),
                    );
                }

                // Recurse into each sub-pattern for its own type-checking / binding introduction.
                for (i, sub) in subs.iter().enumerate() {
                    let field_type = var_info
                        .fields
                        .get(i)
                        .map(|f| f.type_.clone())
                        .unwrap_or(Type::Unknown);
                    self.check_pattern(sub, &field_type, scope, diagnostics)?;
                }

                // A variant pattern is refutable on its own; whether the variant is fully covered
                // (across all arms, including nested sub-patterns) is decided in `check_exhaustiveness`.
                Ok(PatternInfo { irrefutable: false })
            }
        }
    }
    pub(super) fn validate_variant_payload(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        field_types: &[Type],
        arg_types: &[Type],
        position: crate::text::text_span::TextSpan,
        diagnostics: &mut DiagnosticBag,
    ) {
        if arg_types.len() != field_types.len() {
            diagnostics.report_error(
                format!(
                    "Variant '{}.{}' expects {} argument(s), but {} were given",
                    enum_name,
                    variant_name,
                    field_types.len(),
                    arg_types.len()
                ),
                Some(position),
            );
        }

        let expected_strs: Vec<String> = field_types.iter().map(|t| t.get_type()).collect();
        let given_strs: Vec<String> = arg_types.iter().map(|t| t.get_type()).collect();

        self.validate_arguments(
            &format!("Variant '{}.{}'", enum_name, variant_name),
            &expected_strs,
            &given_strs,
            position,
            diagnostics,
        );
    }

    pub(super) fn check_exhaustiveness(
        &self,
        subject_base: &str,
        subject_type: &Type,
        union_info: &Option<UnionInfo>,
        arms: &[SwitchArm<'a>],
        position: Option<crate::text::text_span::TextSpan>,
        diagnostics: &mut DiagnosticBag,
    ) {
        // Only unguarded arms contribute to coverage (a guard may fail at runtime).
        let patterns: Vec<&PatternNode> = arms
            .iter()
            .filter(|a| a.guard.is_none())
            .map(|a| &a.pattern)
            .collect();

        // An irrefutable pattern (`_` or a whole-subject binding) covers everything.
        if patterns
            .iter()
            .any(|p| self.pattern_is_irrefutable(p, subject_type))
        {
            return;
        }

        if let Some(info) = union_info {
            // A variant is covered when a matching arm reaches it and (recursively) its payload
            // sub-patterns cover the field types — so `Wrap(A(n))` + `Wrap(B)` together cover `Wrap`.
            let missing: Vec<String> = info
                .variants
                .iter()
                .filter(|v| !self.variant_covered(&v.name, &v.fields, &patterns))
                .map(|v| v.name.clone())
                .collect();
            if !missing.is_empty() {
                diagnostics.report_error(
                    format!(
                        "Non-exhaustive switch on '{}': missing variant(s) {}. Add the missing arm(s) or a `_` arm",
                        subject_base,
                        missing.join(", ")
                    ),
                    position,
                );
            }
        } else if !subject_type.is_unknown() {
            diagnostics.report_error(
                format!(
                    "Non-exhaustive switch on '{}': add a `_` arm to cover all cases",
                    subject_base
                ),
                position,
            );
        }
    }

    /// True when the set of `patterns` (matched against a value of type `ty`) exhaustively covers
    /// every value of `ty`. Recurses into a union's variants and their single-field payloads, so a
    /// nested match like `Wrap(A(n))` + `Wrap(B)` is recognized as complete. Sound but conservative:
    /// a multi-field variant is only "covered" by an arm whose sub-patterns are all irrefutable
    /// (cartesian coverage across fields is not attempted), and an un-instantiated / non-union field
    /// type needs an irrefutable sub-pattern.
    fn patterns_exhaustive(&self, ty: &Type, patterns: &[&PatternNode]) -> bool {
        if patterns.iter().any(|p| self.pattern_is_irrefutable(p, ty)) {
            return true;
        }
        let base = strip_nullable(&ty.get_type()).to_string();
        let Some(info) = self.union_table.get(&base).cloned() else {
            return false;
        };
        info.variants
            .iter()
            .all(|v| self.variant_covered(&v.name, &v.fields, patterns))
    }

    /// True when `patterns` cover the union variant named `vname` (with `fields` payload).
    fn variant_covered(
        &self,
        vname: &str,
        fields: &[crate::semantics::union_table::UnionFieldInfo],
        patterns: &[&PatternNode],
    ) -> bool {
        match fields.len() {
            // Unit variant: covered by a matching unit pattern (`V` or `V()`).
            0 => patterns
                .iter()
                .any(|p| Self::matches_unit_variant(p, vname)),
            // Single-field variant: covered when the sub-patterns at that field (gathered across all
            // arms matching this variant) recursively cover the field's type.
            1 => {
                let subs: Vec<&PatternNode> = patterns
                    .iter()
                    .filter_map(|p| Self::variant_sub(p, vname, 0))
                    .collect();
                !subs.is_empty() && self.patterns_exhaustive(&fields[0].type_, &subs)
            }
            // Multi-field variant: covered only by an arm binding every field irrefutably.
            _ => patterns
                .iter()
                .any(|p| self.variant_all_irrefutable(p, vname, fields)),
        }
    }

    /// True when `p` always matches a value of type `ty`: `_`, or a bare binding that names no unit
    /// variant of `ty`'s union (a unit-variant binding is refutable — it only matches that variant).
    fn pattern_is_irrefutable(&self, p: &PatternNode, ty: &Type) -> bool {
        match p {
            PatternNode::Wildcard(_) => true,
            PatternNode::Binding(name) => {
                let base = strip_nullable(&ty.get_type()).to_string();
                if let Some(info) = self.union_table.get(&base) {
                    if let Some(v) = info.variant(&name.text) {
                        if v.fields.is_empty() {
                            return false;
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// True when `p` is a unit-variant pattern for `vname` (`V` as a bare binding, or `V()`).
    fn matches_unit_variant(p: &PatternNode, vname: &str) -> bool {
        match p {
            PatternNode::Binding(name) => name.text == vname,
            PatternNode::Variant(_, name, subs) => name.text == vname && subs.is_empty(),
            _ => false,
        }
    }

    /// If `p` is a variant pattern for `vname`, returns its `i`-th sub-pattern.
    fn variant_sub<'p>(p: &'p PatternNode, vname: &str, i: usize) -> Option<&'p PatternNode> {
        match p {
            PatternNode::Variant(_, name, subs) if name.text == vname => subs.get(i),
            _ => None,
        }
    }

    /// True when `p` matches `vname` binding every field irrefutably (e.g. `Pair(a, b)` / `Pair(_, _)`).
    fn variant_all_irrefutable(
        &self,
        p: &PatternNode,
        vname: &str,
        fields: &[crate::semantics::union_table::UnionFieldInfo],
    ) -> bool {
        match p {
            PatternNode::Variant(_, name, subs)
                if name.text == vname && subs.len() == fields.len() =>
            {
                subs.iter()
                    .zip(fields)
                    .all(|(s, f)| self.pattern_is_irrefutable(s, &f.type_))
            }
            _ => false,
        }
    }
}
