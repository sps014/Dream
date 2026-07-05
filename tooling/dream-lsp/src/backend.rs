//! The `tower_lsp` server: owns per-document state and translates protocol requests into queries
//! over the symbol [`Index`] and the diagnostics front-end.
//!
//! Three things make it "seamless": documents are synced **incrementally** (only the changed
//! range is applied), the built [`Index`] is **cached per document version** so repeated
//! navigation requests on an unchanged document are free, and `publishDiagnostics` is
//! **debounced** so a burst of keystrokes only triggers one analysis pass.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tower_lsp::lsp_types::*;
use tower_lsp::{jsonrpc::Result, Client, LanguageServer};

use crate::analysis;
use crate::conversions::{completion_kind, map_position, map_range, symbol_kind};
use crate::index::{self, Index};
use crate::position::LineIndex;
use crate::semantic_tokens;

/// How long to wait after the last edit before publishing diagnostics. A newer edit arriving
/// within the window cancels the pending pass.
const DIAGNOSTIC_DEBOUNCE: Duration = Duration::from_millis(200);

/// The current contents and version of one open document.
#[derive(Debug, Clone)]
struct Document {
    text: String,
    version: i32,
}

/// A symbol index cached against the document version it was built from.
#[derive(Debug, Clone)]
struct CachedIndex {
    version: i32,
    index: Arc<Index>,
}

#[derive(Debug)]
pub struct Backend {
    client: Client,
    documents: Arc<DashMap<String, Document>>,
    index_cache: Arc<DashMap<String, CachedIndex>>,
    /// The most recently scheduled diagnostics version per document, used to debounce/cancel
    /// superseded passes.
    pending_diagnostics: Arc<DashMap<String, i32>>,
}

impl Backend {
    pub fn new(client: Client) -> Backend {
        Backend {
            client,
            documents: Arc::new(DashMap::new()),
            index_cache: Arc::new(DashMap::new()),
            pending_diagnostics: Arc::new(DashMap::new()),
        }
    }

    fn file_path_of(uri: &Url) -> Option<String> {
        uri.to_file_path()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }

    /// Returns the current text of a document, if open.
    fn document_text(&self, uri: &str) -> Option<String> {
        self.documents.get(uri).map(|d| d.text.clone())
    }

    /// Returns the symbol index for a document, rebuilding it only when the cached version is
    /// stale (or absent). The result is shared via [`Arc`] so callers never clone the model.
    fn index_for(&self, uri: &str, file_path: Option<&str>) -> Option<Arc<Index>> {
        let (text, version) = {
            let doc = self.documents.get(uri)?;
            if let Some(cached) = self.index_cache.get(uri) {
                if cached.version == doc.version {
                    return Some(cached.index.clone());
                }
            }
            (doc.text.clone(), doc.version)
        };

        let index = Arc::new(Index::build(file_path, &text));
        self.index_cache.insert(
            uri.to_string(),
            CachedIndex {
                version,
                index: index.clone(),
            },
        );
        Some(index)
    }

    /// Schedules a debounced diagnostics pass for `uri` at `version`. If a newer version is
    /// scheduled before the debounce elapses, this pass is dropped.
    fn schedule_diagnostics(&self, uri: Url, text: String, version: i32) {
        let key = uri.to_string();
        self.pending_diagnostics.insert(key.clone(), version);

        let client = self.client.clone();
        let pending = self.pending_diagnostics.clone();
        let file_path = Self::file_path_of(&uri);

        tokio::spawn(async move {
            tokio::time::sleep(DIAGNOSTIC_DEBOUNCE).await;
            // Bail out if a newer edit superseded this pass while we were waiting.
            if pending.get(&key).map(|v| *v) != Some(version) {
                return;
            }
            let diagnostics = compute_diagnostics(file_path.as_deref(), &text);
            client
                .publish_diagnostics(uri, diagnostics, Some(version))
                .await;
        });
    }
}

/// Runs the front-end and maps its output to protocol diagnostics.
fn compute_diagnostics(file_path: Option<&str>, text: &str) -> Vec<Diagnostic> {
    analysis::collect_diagnostics(file_path, text)
        .into_iter()
        .map(|d| Diagnostic {
            range: map_range(d.range),
            severity: match d.severity {
                "error" => Some(DiagnosticSeverity::ERROR),
                "warning" => Some(DiagnosticSeverity::WARNING),
                _ => Some(DiagnosticSeverity::INFORMATION),
            },
            message: d.message,
            ..Default::default()
        })
        .collect()
}

/// Applies a single content change to `text`. A change with no range is a full-document
/// replacement; otherwise only the spanned bytes are replaced. Offsets are recomputed per change
/// because changes in one notification apply sequentially.
fn apply_change(text: &mut String, range: Option<Range>, new_text: &str) {
    match range {
        None => *text = new_text.to_string(),
        Some(range) => {
            let line_index = LineIndex::new(text);
            let start = line_index
                .offset(range.start.line, range.start.character)
                .min(text.len());
            let end = line_index
                .offset(range.end.line, range.end.character)
                .min(text.len());
            if start <= end {
                text.replace_range(start..end, new_text);
            }
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        ".".to_string(),
                        "\"".to_string(),
                        "/".to_string(),
                    ]),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: semantic_tokens::TOKEN_TYPES.to_vec(),
                                token_modifiers: vec![],
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..Default::default()
                        },
                    ),
                ),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Dream LSP initialized!")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text;
        let version = params.text_document.version;
        self.documents.insert(
            uri.to_string(),
            Document {
                text: text.clone(),
                version,
            },
        );
        self.schedule_diagnostics(uri, text, version);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let key = uri.to_string();

        let text = {
            let mut entry = self
                .documents
                .entry(key.clone())
                .or_insert_with(|| Document {
                    text: String::new(),
                    version: 0,
                });

            for change in params.content_changes {
                apply_change(&mut entry.text, change.range, &change.text);
            }
            entry.version = version;
            entry.text.clone()
        };

        self.schedule_diagnostics(uri, text, version);
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.documents.remove(&uri);
        self.index_cache.remove(&uri);
        self.pending_diagnostics.remove(&uri);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let line_index = LineIndex::new(&text);
        let offset = line_index.offset(
            params.text_document_position_params.position.line,
            params.text_document_position_params.position.character,
        );
        let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
            return Ok(None);
        };
        if let Some(located) = idx.hover(offset) {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: located.contents,
                }),
                range: Some(Range {
                    start: map_position(line_index.position(located.start)),
                    end: map_position(line_index.position(located.end)),
                }),
            }));
        }
        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let line_index = LineIndex::new(&text);
        let offset = line_index.offset(
            params.text_document_position_params.position.line,
            params.text_document_position_params.position.character,
        );
        let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
            return Ok(None);
        };
        if let Some((start, end)) = idx.definition(offset) {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri,
                range: Range {
                    start: map_position(line_index.position(start)),
                    end: map_position(line_index.position(end)),
                },
            })));
        }
        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let line_index = LineIndex::new(&text);
        let offset = line_index.offset(
            params.text_document_position.position.line,
            params.text_document_position.position.character,
        );
        let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
            return Ok(None);
        };
        let locations = idx
            .references(offset, params.context.include_declaration)
            .into_iter()
            .map(|(start, end)| Location {
                uri: uri.clone(),
                range: Range {
                    start: map_position(line_index.position(start)),
                    end: map_position(line_index.position(end)),
                },
            })
            .collect();
        Ok(Some(locations))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri.clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let line_index = LineIndex::new(&text);
        let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
            return Ok(None);
        };
        let symbols = idx
            .document_symbols()
            .into_iter()
            .map(|d| {
                let range = Range {
                    start: map_position(line_index.position(d.start)),
                    end: map_position(line_index.position(d.end)),
                };
                // `DocumentSymbol::deprecated` is a deprecated field in the external `lsp-types`
                // crate; we must still initialize it, so the allow is unavoidable (not our API).
                #[allow(deprecated)]
                DocumentSymbol {
                    name: d.name.clone(),
                    detail: Some(d.detail.clone()),
                    kind: symbol_kind(d.kind),
                    tags: None,
                    deprecated: None,
                    range,
                    selection_range: range,
                    children: None,
                }
            })
            .collect();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query;
        let mut out = Vec::new();
        // Workspace symbols are gathered from every currently-open document. Each document's index
        // is version-cached, so repeated lookups are cheap.
        let keys: Vec<String> = self.documents.iter().map(|e| e.key().clone()).collect();
        for key in keys {
            let Ok(uri) = Url::parse(&key) else {
                continue;
            };
            let Some(text) = self.document_text(&key) else {
                continue;
            };
            let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
                continue;
            };
            let line_index = LineIndex::new(&text);
            for d in idx.symbols_matching(&query) {
                let range = Range {
                    start: map_position(line_index.position(d.start)),
                    end: map_position(line_index.position(d.end)),
                };
                // `SymbolInformation::deprecated` is a deprecated field in `lsp-types` that must
                // still be initialized; the allow is unavoidable (not our API).
                #[allow(deprecated)]
                out.push(SymbolInformation {
                    name: d.name.clone(),
                    kind: symbol_kind(d.kind),
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: uri.clone(),
                        range,
                    },
                    container_name: None,
                });
            }
        }
        Ok(Some(out))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri.clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let line_index = LineIndex::new(&text);
        let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
            return Ok(None);
        };

        let mut hints = Vec::new();
        for hint in &idx.inlay_hints {
            let pos = line_index.position(hint.offset);
            // Type hints (`: int`) sit after the name with left padding; parameter-name hints
            // (`x:`) sit before the argument with right padding.
            let (kind, padding_left, padding_right) = match hint.kind {
                index::InlayKind::Type => (InlayHintKind::TYPE, Some(true), None),
                index::InlayKind::Parameter => (InlayHintKind::PARAMETER, None, Some(true)),
            };
            hints.push(InlayHint {
                position: map_position(pos),
                label: InlayHintLabel::String(hint.label.clone()),
                kind: Some(kind),
                text_edits: None,
                tooltip: None,
                padding_left,
                padding_right,
                data: None,
            });
        }
        Ok(Some(hints))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let file_path = Self::file_path_of(&uri);
        let line_index = LineIndex::new(&text);
        let offset = line_index.offset(
            params.text_document_position.position.line,
            params.text_document_position.position.character,
        );
        let Some(idx) = self.index_for(&key, file_path.as_deref()) else {
            return Ok(None);
        };
        let completions = idx.completions(file_path.as_deref(), &text, offset);

        let items: Vec<CompletionItem> = completions
            .into_iter()
            .map(|(label, kind, detail, doc_comment)| CompletionItem {
                label,
                kind: Some(completion_kind(kind)),
                detail: Some(detail),
                documentation: doc_comment.map(|doc| {
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc,
                    })
                }),
                ..Default::default()
            })
            .collect();
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let line_index = LineIndex::new(&text);
        let offset = line_index.offset(
            params.text_document_position_params.position.line,
            params.text_document_position_params.position.character,
        );
        let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
            return Ok(None);
        };
        if let Some(decl) = idx.signature_help(&text, offset) {
            let label = decl.detail.clone();
            let mut parameters = vec![];

            if let Some(start_paren) = label.find('(') {
                if let Some(end_paren) = label.rfind(')') {
                    if start_paren < end_paren {
                        let params_str = &label[start_paren + 1..end_paren];
                        if !params_str.trim().is_empty() {
                            for param in params_str.split(',') {
                                parameters.push(ParameterInformation {
                                    label: ParameterLabel::Simple(param.trim().to_string()),
                                    documentation: None,
                                });
                            }
                        }
                    }
                }
            }

            let active_parameter = active_parameter_at(&text, offset);

            return Ok(Some(SignatureHelp {
                signatures: vec![SignatureInformation {
                    label,
                    documentation: decl.doc_comment.map(|doc| {
                        Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: doc,
                        })
                    }),
                    parameters: Some(parameters),
                    active_parameter: Some(active_parameter),
                }],
                active_signature: Some(0),
                active_parameter: Some(active_parameter),
            }));
        }
        Ok(None)
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let key = params.text_document.uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let formatted = crate::format::format(&text);
        let line_index = LineIndex::new(&text);
        let end_pos = line_index.position(text.len());
        Ok(Some(vec![TextEdit {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: map_position(end_pos),
            },
            new_text: formatted,
        }]))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri.clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let tokens = semantic_tokens::compute(Self::file_path_of(&uri).as_deref(), &text);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri.clone();
        let key = uri.to_string();
        let Some(text) = self.document_text(&key) else {
            return Ok(None);
        };
        let line_index = LineIndex::new(&text);
        let Some(idx) = self.index_for(&key, Self::file_path_of(&uri).as_deref()) else {
            return Ok(None);
        };

        let mut lenses = Vec::new();
        // Look for a top-level function named "main"
        for decl in &idx.decls {
            if decl.name == "main" && decl.kind == index::SymKind::Function {
                // The range points to the start of the 'fun main' token
                let range = Range {
                    start: map_position(line_index.position(decl.start)),
                    end: map_position(line_index.position(decl.end)),
                };

                // Add Run CodeLens
                lenses.push(CodeLens {
                    range,
                    command: Some(Command {
                        title: "▶ Run".to_string(),
                        command: "dream.runFile".to_string(),
                        arguments: None,
                    }),
                    data: None,
                });

                // Add Debug CodeLens
                lenses.push(CodeLens {
                    range,
                    command: Some(Command {
                        title: "▶ Debug".to_string(),
                        command: "dream.debugFile".to_string(),
                        arguments: None,
                    }),
                    data: None,
                });
            }
        }

        Ok(Some(lenses))
    }
}

/// Counts the comma-separated argument the cursor sits in, by scanning back to the opening paren
/// of the current call (skipping nested parens). Used to highlight the active parameter.
fn active_parameter_at(text: &str, offset: usize) -> u32 {
    let bytes = text.as_bytes();
    let mut active_parameter = 0;
    let mut i = offset;
    let mut paren_count = 0;
    while i > 0 {
        i -= 1;
        let b = bytes[i];
        if b == b')' {
            paren_count += 1;
        } else if b == b'(' {
            if paren_count > 0 {
                paren_count -= 1;
            } else {
                break;
            }
        } else if b == b',' && paren_count == 0 {
            active_parameter += 1;
        } else if b == b';' || b == b'{' || b == b'}' {
            break;
        }
    }
    active_parameter
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Position, Range};

    #[test]
    fn test_apply_change_full_document() {
        let mut text = "hello world".to_string();
        apply_change(&mut text, None, "goodbye");
        assert_eq!(text, "goodbye");
    }

    #[test]
    fn test_apply_change_incremental() {
        let mut text = "hello world\nnew line".to_string();
        // Replace "world" with "there"
        let range = Range {
            start: Position {
                line: 0,
                character: 6,
            },
            end: Position {
                line: 0,
                character: 11,
            },
        };
        apply_change(&mut text, Some(range), "there");
        assert_eq!(text, "hello there\nnew line");
    }

    #[test]
    fn test_apply_change_multi_line() {
        let mut text = "line 1\nline 2\nline 3".to_string();
        // Replace from end of line 1 to start of line 3
        let range = Range {
            start: Position {
                line: 0,
                character: 6,
            },
            end: Position {
                line: 2,
                character: 0,
            },
        };
        apply_change(&mut text, Some(range), " inserted ");
        assert_eq!(text, "line 1 inserted line 3");
    }
}
