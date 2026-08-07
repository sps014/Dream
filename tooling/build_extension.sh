#!/usr/bin/env bash
# Builds and packages the Dream VS Code extension (.vsix).
# The extension does not bundle dream / dream-lsp / dreamer — point it at a local toolchain
# with `source ./use-toolchain.sh` or settings dream.home / dreamer.home.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VSCODE_DIR="$SCRIPT_DIR/vscode"

echo "==> Navigating to VS Code extension directory..."
cd "$VSCODE_DIR"

echo "==> Installing dependencies..."
npm install

echo "==> Compiling TypeScript..."
npm run compile

echo "==> Packaging extension into .vsix..."
npx @vscode/vsce package

echo "==> Done! You can install the extension with:"
echo "    code --install-extension tooling/vscode/$(ls -t *.vsix | head -n 1)"
echo ""
echo "Before using it, make the toolchain available:"
echo "    source ./use-toolchain.sh"
echo "    # or set VS Code settings dream.home / dreamer.home"
