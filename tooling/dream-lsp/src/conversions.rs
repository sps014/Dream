//! Conversions between this crate's internal models ([`crate::position`], [`crate::index`]) and
//! the `tower_lsp` protocol types. Kept in one place so the protocol layer stays thin.

use tower_lsp::lsp_types::{
    CompletionItemKind, Position as LspPosition, Range as LspRange, SymbolKind,
};

use crate::index::SymKind;
use crate::position::{Position, Range};

pub fn map_position(pos: Position) -> LspPosition {
    LspPosition {
        line: pos.line,
        character: pos.character,
    }
}

pub fn map_range(range: Range) -> LspRange {
    LspRange {
        start: map_position(range.start),
        end: map_position(range.end),
    }
}

/// Maps a symbol kind to the completion-item icon shown in the editor's completion list.
pub fn completion_kind(kind: SymKind) -> CompletionItemKind {
    match kind {
        SymKind::Function => CompletionItemKind::FUNCTION,
        SymKind::Class => CompletionItemKind::CLASS,
        SymKind::Struct => CompletionItemKind::STRUCT,
        SymKind::Interface => CompletionItemKind::INTERFACE,
        SymKind::Enum => CompletionItemKind::ENUM,
        SymKind::EnumMember => CompletionItemKind::ENUM_MEMBER,
        SymKind::Field => CompletionItemKind::FIELD,
        SymKind::Method => CompletionItemKind::METHOD,
        SymKind::Variable | SymKind::Param => CompletionItemKind::VARIABLE,
        SymKind::Type => CompletionItemKind::CLASS,
        SymKind::Keyword => CompletionItemKind::KEYWORD,
        SymKind::Decorator => CompletionItemKind::KEYWORD,
    }
}

/// Maps a symbol kind to the document-outline symbol kind.
pub fn symbol_kind(kind: SymKind) -> SymbolKind {
    match kind {
        SymKind::Function => SymbolKind::FUNCTION,
        SymKind::Class => SymbolKind::CLASS,
        SymKind::Struct => SymbolKind::STRUCT,
        SymKind::Interface => SymbolKind::INTERFACE,
        SymKind::Enum => SymbolKind::ENUM,
        SymKind::EnumMember => SymbolKind::ENUM_MEMBER,
        SymKind::Field => SymbolKind::FIELD,
        SymKind::Method => SymbolKind::METHOD,
        SymKind::Variable | SymKind::Param => SymbolKind::VARIABLE,
        SymKind::Type => SymbolKind::CLASS,
        SymKind::Keyword => SymbolKind::KEY,
        SymKind::Decorator => SymbolKind::KEY,
    }
}
