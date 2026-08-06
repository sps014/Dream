//! GeneratorContext: discovery helpers + emit/replace + diagnostics.

use super::parse_extends;
use super::registration::RegisteredGenerator;
use super::rewrite::rewrite_function;
use super::semantic::{SemanticModel, Symbol, TypeSymbol};
use super::syntax::{SyntaxNodeId, SyntaxTreeView};
use bumpalo::Bump;
use dream_diagnostics::DiagnosticBag;
use indexmap::IndexMap;
use std::io::Error;

use crate::driver::source_loader::ProgramAccumulator;

pub struct GeneratorContext {
    pub semantic: SemanticModel,
    pub syntax: SyntaxTreeView,
    pub registered: Vec<RegisteredGenerator>,
    pub syntax_block_names: Vec<String>,
    emits: Vec<EmitRequest>,
    replacements: IndexMap<SyntaxNodeId, String>,
    errors: Vec<(Option<dream_text::text_span::TextSpan>, String)>,
}

enum EmitRequest {
    Extend { type_name: String, body: String },
    File { path: String, source: String },
}

impl GeneratorContext {
    pub fn build(acc: &ProgramAccumulator<'_>, registered: Vec<RegisteredGenerator>) -> Self {
        let semantic = SemanticModel::from_program(
            &acc.all_structs,
            &acc.all_enums,
            &acc.all_interfaces,
            &acc.all_functions,
        );
        let mut syntax = SyntaxTreeView::default();
        for f in &acc.all_functions {
            for s in f.body {
                syntax.walk_stmt_public(s);
            }
        }
        for st in &acc.all_structs {
            for m in &st.methods {
                for s in m.body {
                    syntax.walk_stmt_public(s);
                }
            }
        }
        for e in &acc.all_extends {
            for m in &e.methods {
                for s in m.body {
                    syntax.walk_stmt_public(s);
                }
            }
        }
        for g in &acc.all_globals {
            syntax.walk_expr_public(&g.initializer);
        }

        let mut syntax_block_names: Vec<String> = registered
            .iter()
            .flat_map(|r| r.syntax_blocks.iter().cloned())
            .collect();
        syntax_block_names.sort();
        syntax_block_names.dedup();

        Self {
            semantic,
            syntax,
            registered,
            syntax_block_names,
            emits: Vec::new(),
            replacements: IndexMap::new(),
            errors: Vec::new(),
        }
    }

    pub fn types(&self) -> Vec<&TypeSymbol> {
        self.semantic.types().collect()
    }

    pub fn types_with(&self, attr: &str) -> Vec<&TypeSymbol> {
        self.semantic.types_with(attr)
    }

    pub fn functions_with(&self, attr: &str) -> Vec<&Symbol> {
        self.semantic.functions_with(attr)
    }

    pub fn syntax_blocks(&self, name: &str) -> Vec<SyntaxNodeId> {
        self.syntax.syntax_blocks(name)
    }

    pub fn error(&mut self, node: SyntaxNodeId, message: impl Into<String>) {
        let span = self.syntax.get(node).and_then(|n| n.span);
        self.errors.push((span, message.into()));
    }

    pub fn error_at(&mut self, _file: &str, _line: i32, message: impl Into<String>) {
        self.errors.push((None, message.into()));
    }

    pub fn emit_file(&mut self, synthetic_path: impl Into<String>, dream_source: impl Into<String>) {
        self.emits.push(EmitRequest::File {
            path: synthetic_path.into(),
            source: dream_source.into(),
        });
    }

    pub fn emit_extend(&mut self, type_name: impl Into<String>, body_source: impl Into<String>) {
        self.emits.push(EmitRequest::Extend {
            type_name: type_name.into(),
            body: body_source.into(),
        });
    }

    pub fn replace(&mut self, node: SyntaxNodeId, dream_source: impl Into<String>) {
        self.replacements.insert(node, dream_source.into());
    }

    pub fn flush_errors(&mut self, diagnostics: &mut DiagnosticBag) {
        for (span, msg) in self.errors.drain(..) {
            diagnostics.report_error(msg, span);
        }
    }

    pub fn apply_emits<'a>(
        &mut self,
        arena: &'a Bump,
        acc: &mut ProgramAccumulator<'a>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), Error> {
        self.flush_errors(diagnostics);
        let mut combined_extends = String::new();
        for emit in self.emits.drain(..) {
            match emit {
                EmitRequest::Extend { type_name, body } => {
                    combined_extends.push_str("extend ");
                    combined_extends.push_str(&type_name);
                    combined_extends.push_str(" {\n");
                    combined_extends.push_str(&body);
                    if !body.ends_with('\n') {
                        combined_extends.push('\n');
                    }
                    combined_extends.push_str("}\n\n");
                }
                EmitRequest::File { path, source } => {
                    let extends =
                        parse_extends(arena, source, &path, diagnostics, &mut acc.file_contents)?;
                    acc.all_extends.extend(extends);
                }
            }
        }
        if !combined_extends.is_empty() {
            let extends = parse_extends(
                arena,
                combined_extends,
                "<generate-extends>",
                diagnostics,
                &mut acc.file_contents,
            )?;
            acc.all_extends.extend(extends);
        }
        Ok(())
    }

    pub fn apply_replacements<'a>(
        &mut self,
        arena: &'a Bump,
        acc: &mut ProgramAccumulator<'a>,
        diagnostics: &mut DiagnosticBag,
    ) -> Result<(), Error> {
        self.flush_errors(diagnostics);
        if self.replacements.is_empty() {
            return Ok(());
        }
        let mut by_site: IndexMap<(String, String), String> = IndexMap::new();
        for (id, source) in &self.replacements {
            if let Some(site) = self.syntax.block_keys.get(id) {
                by_site.insert((site.name.clone(), site.body_text.clone()), source.clone());
            }
        }

        for f in &mut acc.all_functions {
            rewrite_function(arena, f, &by_site, diagnostics)?;
        }
        for st in &mut acc.all_structs {
            for m in &mut st.methods {
                rewrite_function(arena, m, &by_site, diagnostics)?;
            }
        }
        for e in &mut acc.all_extends {
            for m in &mut e.methods {
                rewrite_function(arena, m, &by_site, diagnostics)?;
            }
        }
        Ok(())
    }
}
