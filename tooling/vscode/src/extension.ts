import * as path from 'path';
import * as vscode from 'vscode';
import * as fs from 'fs';
import { exec } from 'child_process';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions
} from 'vscode-languageclient/node';

let client: LanguageClient;
let runTerminal: vscode.Terminal | undefined;
let watPanel: vscode.WebviewPanel | undefined;
let compilerOutputChannel: vscode.OutputChannel;

/**
 * Resolves a shell command prefix for invoking the bundled `dream` compiler CLI binary.
 * Returns `null` (and shows an error message) if no bundled binary is found for this
 * platform/arch, rather than falling back to building it from source.
 */
function resolveDreamCliCommand(context: vscode.ExtensionContext): string | null {
    const platform = process.platform;
    const arch = process.arch;
    const ext = platform === 'win32' ? '.exe' : '';

    const specificBinName = `dream-${platform}-${arch}${ext}`;
    const genericBinName = `dream${ext}`;

    const specificBinPath = path.join(context.extensionPath, 'bin', specificBinName);
    const genericBinPath = path.join(context.extensionPath, 'bin', genericBinName);

    let binPath = '';
    if (fs.existsSync(specificBinPath)) {
        binPath = specificBinPath;
    } else if (fs.existsSync(genericBinPath)) {
        binPath = genericBinPath;
    }

    if (binPath === '') {
        vscode.window.showErrorMessage(
            `Dream: no bundled compiler binary found for ${platform}-${arch} (expected "${specificBinName}" or "${genericBinName}" in the extension's bin/ folder).`
        );
        return null;
    }

    try {
        fs.chmodSync(binPath, '755');
    } catch {
        // Best-effort; if this fails the subsequent invocation will surface the real error.
    }
    return `"${binPath}"`;
}

/** Escapes a path for safe interpolation inside a double-quoted shell argument. */
function quotePath(filePath: string): string {
    return `"${filePath.replace(/"/g, '\\"')}"`;
}

/** Derives the sibling `.wat` path that the compiler writes next to a `.dream` source file. */
function watPathFor(filePath: string): string {
    const parsed = path.parse(filePath);
    return path.join(parsed.dir, `${parsed.name}.wat`);
}

function escapeHtml(text: string): string {
    return text
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
}

async function saveActiveDreamFile(editor: vscode.TextEditor): Promise<void> {
    if (editor.document.isDirty) {
        await editor.document.save();
    }
}

function registerRunFileCommand(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
        vscode.commands.registerCommand('dream.runFile', async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'dream') {
                vscode.window.showWarningMessage('Open a .dream file to run it.');
                return;
            }

            await saveActiveDreamFile(editor);

            const dreamCmd = resolveDreamCliCommand(context);
            if (!dreamCmd) {
                return;
            }
            const filePath = editor.document.uri.fsPath;

            if (!runTerminal || runTerminal.exitStatus !== undefined) {
                runTerminal = vscode.window.createTerminal('Dream');
            }
            runTerminal.show();
            runTerminal.sendText(`${dreamCmd} run ${quotePath(filePath)}`);
        })
    );
}

function registerShowWatCommand(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
        vscode.commands.registerCommand('dream.showWat', async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'dream') {
                vscode.window.showWarningMessage('Open a .dream file to view its generated WAT.');
                return;
            }

            await saveActiveDreamFile(editor);

            const dreamCmd = resolveDreamCliCommand(context);
            if (!dreamCmd) {
                return;
            }
            const filePath = editor.document.uri.fsPath;
            const watPath = watPathFor(filePath);
            const fileLabel = path.basename(filePath);

            const command = `${dreamCmd} ${quotePath(filePath)}`;
            exec(command, { cwd: path.dirname(filePath) }, (error, stdout, stderr) => {
                if (error) {
                    const details = [stderr, stdout].filter(Boolean).join('\n');
                    compilerOutputChannel.appendLine(`--- Compile failed: ${fileLabel} ---`);
                    if (details) {
                        compilerOutputChannel.appendLine(details);
                    } else {
                        compilerOutputChannel.appendLine(String(error));
                    }
                    compilerOutputChannel.show(true);
                    vscode.window.showErrorMessage(
                        `Dream: failed to compile ${fileLabel}. See "Dream Compiler" output for details.`
                    );
                    return;
                }

                let watContent: string;
                try {
                    watContent = fs.readFileSync(watPath, 'utf8');
                } catch (readErr) {
                    vscode.window.showErrorMessage(
                        `Dream: compiled successfully but could not read generated WAT at ${watPath}: ${readErr}`
                    );
                    return;
                }

                showWatPanel(fileLabel, watContent);
            });
        })
    );
}

function showWatPanel(fileLabel: string, watContent: string): void {
    if (!watPanel) {
        watPanel = vscode.window.createWebviewPanel(
            'dreamWat',
            `Dream: ${fileLabel}.wat`,
            vscode.ViewColumn.Beside,
            { enableScripts: false }
        );
        watPanel.onDidDispose(() => {
            watPanel = undefined;
        });
    } else {
        watPanel.title = `Dream: ${fileLabel}.wat`;
        watPanel.reveal(vscode.ViewColumn.Beside, true);
    }

    watPanel.webview.html = `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<style>
  body {
    padding: 0;
    margin: 0;
    background-color: var(--vscode-editor-background);
    color: var(--vscode-editor-foreground);
  }
  pre {
    margin: 0;
    padding: 12px 16px;
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: var(--vscode-editor-font-size, 13px);
    white-space: pre;
    overflow-x: auto;
  }
</style>
</head>
<body>
<pre>${escapeHtml(watContent)}</pre>
</body>
</html>`;
}

export async function activate(context: vscode.ExtensionContext) {
    const outputChannel = vscode.window.createOutputChannel('Dream Language Server');
    outputChannel.appendLine('Activating Dream extension...');

    compilerOutputChannel = vscode.window.createOutputChannel('Dream Compiler');
    context.subscriptions.push(compilerOutputChannel);

    registerRunFileCommand(context);
    registerShowWatCommand(context);

    const platform = process.platform;
    const arch = process.arch;
    const ext = platform === 'win32' ? '.exe' : '';
    
    // Check for platform-specific binary (e.g. dream-lsp-darwin-arm64)
    const specificBinName = `dream-lsp-${platform}-${arch}${ext}`;
    const genericBinName = `dream-lsp${ext}`;
    
    const specificBinPath = path.join(__dirname, '..', 'bin', specificBinName);
    const genericBinPath = path.join(__dirname, '..', 'bin', genericBinName);
    
    let binPath = '';
    if (fs.existsSync(specificBinPath)) {
        binPath = specificBinPath;
    } else if (fs.existsSync(genericBinPath)) {
        binPath = genericBinPath;
    }

    let serverOptions: ServerOptions;

    if (binPath !== '') {
        outputChannel.appendLine(`Found bundled binary at ${binPath}`);
        try {
            fs.chmodSync(binPath, '755');
        } catch (e) {
            outputChannel.appendLine(`Failed to make binary executable: ${e}`);
        }
        serverOptions = {
            command: binPath,
            args: [],
            options: { env: process.env }
        };
    } else {
        outputChannel.appendLine('Bundled binary not found. Falling back to cargo...');
        
        const isCargoAvailable = await new Promise<boolean>((resolve) => {
            exec('cargo --version', (error) => resolve(!error));
        });

        if (!isCargoAvailable) {
            const msg = 'Dream LSP failed to start: "cargo" is not available in your PATH, and no bundled binary was found.';
            vscode.window.showErrorMessage(msg);
            outputChannel.appendLine(msg);
            outputChannel.show();
            return;
        }

        const manifestPath = path.join(__dirname, '..', '..', 'dream-lsp', 'Cargo.toml');
        serverOptions = {
            command: 'cargo',
            args: ['run', '-q', '--manifest-path', manifestPath],
            options: { env: process.env }
        };
    }

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'dream' }],
        outputChannel: outputChannel
    };

    client = new LanguageClient(
        'dreamLanguageServer',
        'Dream Language Server',
        serverOptions,
        clientOptions
    );

    context.subscriptions.push(client);

    try {
        outputChannel.appendLine('Starting client...');
        await client.start();
        outputChannel.appendLine('Client started successfully.');
    } catch (err) {
        outputChannel.appendLine(`Failed to start client: ${err}`);
        vscode.window.showErrorMessage(`Dream LSP failed to start. Check the 'Dream Language Server' output channel for details.`);
        outputChannel.show();
    }
}

export function deactivate(): Thenable<void> | undefined {
    runTerminal?.dispose();
    watPanel?.dispose();
    if (!client) {
        return undefined;
    }
    return client.stop();
}
