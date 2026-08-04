# Dream Tooling

This directory contains developer tooling for the Dream language, focused around the native Language Server Protocol (LSP) and editor extensions.

## Layout

- [`dream-lsp/`](dream-lsp) — A native Rust Language Server Protocol (LSP) server binary powered by `tower-lsp`. It reuses the compiler's frontend (lexer, parser, semantic analyzer) to provide rich IntelliSense features.
- [`vscode/`](vscode) — A Visual Studio Code extension client written in TypeScript that connects to the `dream-lsp` server.
- [`dreamer/`](dreamer) — The Dream package manager: reads/writes `dream.toml` manifests and `dream.lock` lockfiles, resolves and installs dependencies into `dream_packages/`, and wraps the `dream` compiler for `build`/`run`. See [`docs/tooling/package-manager.md`](../docs/tooling/package-manager.md) for the full manifest/registry format and a CLI walkthrough.

## Features Supported

The `dream-lsp` server provides the following capabilities:
- **Real-time Diagnostics**: Reports syntax and semantic errors/warnings directly in the editor. Diagnostics keep flowing even while a document has a syntax error — the parser recovers and the analyzer runs over whatever parsed — and are debounced so a burst of keystrokes triggers a single analysis pass.
- **Semantic Tokens**: AST-driven, perfectly accurate syntax highlighting (functions, classes, fields, parameters, etc.).
- **Autocomplete (IntelliSense)**: Intelligent completions for keywords, data types, and scoped symbols (locals, parameters, top-level `let`/`const` globals, and cross-file imports).
- **Hover**: Rich Markdown hover tooltips displaying symbol signatures and documentation comments (functions, types, members, and top-level globals).
- **Signature Help**: Pop-up parameter hints and active parameter tracking when writing function or constructor calls.
- **Go to Definition / Find References**: Jump to a symbol's declaration or list every usage of it.
- **Document Symbols**: An outline of a file's top-level declarations (functions, types, enum members, fields, methods, and globals).
- **Inlay Hints**: Inferred variable types and parameter-name hints at call sites.
- **Formatting**: Brace-depth indentation.

Documents are synced **incrementally** (only the changed range is applied) and the symbol index is **cached per document version**, so repeated navigation on an unchanged file is free.

## Building and Running the Extension

To test or develop the VS Code extension:

1. Ensure you have Node.js, `npm`, and `cargo` installed.
2. Navigate to the `vscode/` folder:
   ```bash
   cd vscode
   npm install
   npm run compile
   ```
3. You can either open the workspace in VS Code and press **F5** to launch the Extension Development Host, or build a `.vsix` package to install it globally:
   ```bash
   npx @vscode/vsce package
   code --install-extension dream-lang-0.1.0.vsix
   ```

*(Note: The VS Code extension automatically attempts to invoke `cargo run` from the `dream-lsp` crate when starting, so you must have the Rust toolchain installed locally).*

## Testing the LSP Server

The LSP server contains standalone tests to verify the compiler and analysis pipeline works without needing an editor:

```bash
cargo test -p dream-lsp
```