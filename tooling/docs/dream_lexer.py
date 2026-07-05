"""Pygments lexer for the Dream language, registered as an MkDocs hook.

Referenced from ``mkdocs.yml`` via ``hooks:`` so that ```` ```dream ```` code
fences are highlighted with Dream's own grammar instead of a TypeScript
approximation. The lexer is injected into Pygments' registry at import time,
which happens before any page is rendered, so no package install or entry
point is required.
"""

from pygments.lexer import RegexLexer, bygroups, include, words
from pygments.token import (
    Comment,
    Keyword,
    Name,
    Number,
    Operator,
    Punctuation,
    String,
    Whitespace,
)

__all__ = ["DreamLexer"]


class DreamLexer(RegexLexer):
    name = "Dream"
    aliases = ["dream"]
    filenames = ["*.dream"]

    # Keyword groups mirror tooling/vscode/syntaxes/dream.tmLanguage.json, which in turn should
    # match the canonical `KEYWORDS`/`CONTEXTUAL_KEYWORDS` lists in
    # crates/dream-syntax/src/token/token_kind.rs - update all three together when the parser gains
    # a new reserved word.
    _control = (
        "if", "else", "for", "while", "do", "return", "break", "continue",
        "switch", "case", "default", "is", "in", "async", "await",
    )
    _declaration = (
        "let", "const", "class", "struct", "interface", "enum", "type", "extend", "fun",
        "constructor", "del",
    )
    _modifiers = ("static", "public", "extern", "import", "override", "sealed", "unmanaged")
    _builtin_types = (
        "int", "float", "double", "string", "bool", "char", "void", "object",
    )

    tokens = {
        "root": [
            (r"\s+", Whitespace),
            (r"//.*?$", Comment.Single),
            (r"/\*", Comment.Multiline, "comment"),
            # Attributes: @json, @override, @js, @property_name, ...
            (r"@[A-Za-z_]\w*", Name.Decorator),
            # Interpolated string `$"...{expr}..."` (must precede the plain string rule).
            (r'\$"', String.Interpol, "interpstring"),
            (r'"', String.Double, "dqstring"),
            (r"'", String.Char, "sqstring"),
            # Declarations that introduce a named entity.
            (r"\b(fun)(\s+)([A-Za-z_]\w*)",
             bygroups(Keyword.Declaration, Whitespace, Name.Function)),
            # Contextual accessor keywords: `get name(...)` / `set name(...)`. Matched only in
            # accessor position (keyword + property name + `(`) so ordinary calls such as
            # `list.get(0)` keep `get`/`set` as plain identifiers.
            (r"\b(get|set)(\s+)([A-Za-z_]\w*)(?=\s*\()",
             bygroups(Keyword.Declaration, Whitespace, Name.Function)),
            (r"\b(class|enum|extend|type)(\s+)([A-Za-z_]\w*)",
             bygroups(Keyword.Declaration, Whitespace, Name.Class)),
            (words(_control, prefix=r"\b", suffix=r"\b"), Keyword),
            (words(_declaration, prefix=r"\b", suffix=r"\b"), Keyword.Declaration),
            (words(_modifiers, prefix=r"\b", suffix=r"\b"), Keyword.Reserved),
            (r"\b(true|false|null)\b", Keyword.Constant),
            (r"\bthis\b", Name.Builtin.Pseudo),
            (words(_builtin_types, prefix=r"\b", suffix=r"\b"), Keyword.Type),
            (r"\b\d+\.\d+[dDfF]?\b", Number.Float),
            (r"\b\d+[dDfF]\b", Number.Float),
            (r"\b0[xX][0-9a-fA-F]+\b", Number.Hex),
            (r"\b\d+\b", Number.Integer),
            # CapWords are type names (classes, enums, generics).
            (r"\b[A-Z]\w*\b", Name.Class),
            # An identifier immediately followed by `(` is a call.
            (r"\b([A-Za-z_]\w*)(?=\s*\()", Name.Function),
            (r"\+\+|--|\+=|-=|\*=|/=|%=|==|!=|>=|<=|&&|\|\||<<|>>|\?\?"
             r"|[-+*/%=<>!&|^~?:]", Operator),
            (r"[{}()\[\];,.]", Punctuation),
            (r"\b[A-Za-z_]\w*\b", Name),
        ],
        "comment": [
            (r"[^*/]+", Comment.Multiline),
            (r"/\*", Comment.Multiline, "#push"),
            (r"\*/", Comment.Multiline, "#pop"),
            (r"[*/]", Comment.Multiline),
        ],
        "dqstring": [
            (r"\\.", String.Escape),
            (r'"', String.Double, "#pop"),
            (r'[^"\\]+', String.Double),
        ],
        "sqstring": [
            (r"\\.", String.Escape),
            (r"'", String.Char, "#pop"),
            (r"[^'\\]+", String.Char),
        ],
        # `$"..."` body: literal text plus `{expr}` holes. `{{`/`}}` are literal braces.
        "interpstring": [
            (r"\\.", String.Escape),
            (r"\{\{|\}\}", String.Escape),
            (r"\{", String.Interpol, "interp"),
            (r'"', String.Interpol, "#pop"),
            (r'[^"\\{}]+', String.Double),
        ],
        # A single interpolation hole, highlighted as ordinary Dream code until `}`.
        "interp": [
            (r"\}", String.Interpol, "#pop"),
            include("root"),
        ],
    }


def _register_with_pygments():
    """Make ``get_lexer_by_name('dream')`` resolve to :class:`DreamLexer`.

    Pygments reads its ``LEXERS`` mapping and ``_lexer_cache`` lazily inside
    ``get_lexer_by_name``, so populating them here is enough regardless of
    import order relative to pymdownx.
    """
    try:
        import pygments.lexers as pl

        pl.LEXERS["DreamLexer"] = (
            "dream_lexer", "Dream", ("dream",), ("*.dream",), (),
        )
        # Pre-seed the cache so the (non-importable) module name above is
        # never actually loaded.
        pl._lexer_cache["Dream"] = DreamLexer
    except Exception:  # pragma: no cover - highlighting just falls back
        pass


_register_with_pygments()
