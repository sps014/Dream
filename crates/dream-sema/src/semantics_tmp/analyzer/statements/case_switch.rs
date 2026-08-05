//! The C-style `switch`/`case` statement over int/string/bool/enum subjects (distinct from the
//! pattern-matching `switch` in [`super::super::switch_unions`]).

use super::*;
use crate::diagnostics::DiagnosticBag;
use crate::hir::HExpr;
use crate::semantics::errors::SemanticError;
use crate::syntax::nodes::{ExpressionNode, StatementNode, Type};

impl<'a> Analyzer<'a> {
    pub(in crate::semantics::analyzer) fn analyze_case_switch(
        &mut self,
        subject: &ExpressionNode<'a>,
        cases: &Vec<(Vec<ExpressionNode<'a>>, &'a [StatementNode<'a>])>,
        default_body: &Option<&'a [StatementNode<'a>]>,
        ctx: &super::super::AnalyzerContext<'a, '_>,
        has_parent_while: bool,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), SemanticError> {
        let subject_type = self
            .analyze_expression(subject, ctx.parent_function, ctx.symbol_table, diagnostics)
            .unwrap_or(Type::Unknown);
        let subject_hir = self.hir_take();
        let mut hir_arms: Vec<crate::hir::HArm> = Vec::new();
        // A multi-label case (`case 1, 2, 3:`) becomes one `HArm` per label, all sharing a clone of
        // the case body (each label is a distinct dispatch target hitting the same code).
        let mut hir_ok = true;
        let subject_name = subject_type.get_type();
        let subject_is_enum = self.enum_table.contains_key(&subject_name);
        if !matches!(subject_name.as_str(), "int" | "string" | "bool") && !subject_is_enum {
            diagnostics.report_error(
                format!(
                    "switch subject must be int, string, bool, or an enum, got {}",
                    subject_name
                ),
                subject.position(),
            );
        }

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (labels, body) in cases.iter() {
            let mut label_hirs: Vec<Option<HExpr>> = Vec::new();
            for label in labels.iter() {
                // Labels must be compile-time constants: a literal, or (for enum switches) an
                // enum member access like `Color.Red`.
                let is_enum_label = matches!(label, ExpressionNode::MemberAccess(_, _));
                if !matches!(label, ExpressionNode::Literal(_)) && !is_enum_label {
                    diagnostics.report_error(
                        "switch case labels must be constant literals or enum members".to_string(),
                        label.position(),
                    );
                }
                let label_type = self
                    .analyze_expression(label, ctx.parent_function, ctx.symbol_table, diagnostics)
                    .unwrap_or(Type::Unknown);
                label_hirs.push(self.hir_take());
                self.compare_data_type(&subject_type, &label_type, &empty_span(), diagnostics)?;

                let key = match label {
                    ExpressionNode::Literal(
                        Type::Integer(t)
                        | Type::Float(t)
                        | Type::Double(t)
                        | Type::String(t)
                        | Type::Boolean(t),
                    ) => Some(t.text.clone()),
                    ExpressionNode::Literal(_) => None,
                    ExpressionNode::MemberAccess(_, m) => Some(m.text.clone()),
                    _ => None,
                };
                if let Some(k) = key {
                    if !seen.insert(k.clone()) {
                        diagnostics.report_error(
                            format!("duplicate case label '{}' in switch statement", k),
                            label.position(),
                        );
                    }
                }
            }
            self.hir_open_block();
            self.analyze_body(
                body,
                ctx.parent_function,
                Some(ctx.symbol_table),
                has_parent_while,
                diagnostics,
            )?;
            let body_hir = self.hir_close_block();
            // One arm per label; all labels of a case share (a clone of) its body.
            for label_hir in label_hirs {
                match self.hir_const_arm(label_hir, body_hir.clone()) {
                    Some(arm) => hir_arms.push(arm),
                    None => hir_ok = false,
                }
            }
        }

        let default_hir = if let Some(db) = default_body {
            self.hir_open_block();
            self.analyze_body(
                db,
                ctx.parent_function,
                Some(ctx.symbol_table),
                has_parent_while,
                diagnostics,
            )?;
            self.hir_close_block()
        } else {
            Vec::new()
        };

        self.hir_switch(subject_hir, hir_arms, default_hir, hir_ok);
        Ok(())
    }
}
