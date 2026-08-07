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

type BuildMode = 'debug' | 'release';
type OptimizeLevel = 'default' | '0' | '1' | '2' | '3' | '4' | 's' | 'z';
type RuntimeTarget = 'native' | 'web' | 'node';

interface DreamBuildSettings {
    buildMode: BuildMode;
    optimizeLevel: OptimizeLevel;
    runtimeTarget: RuntimeTarget;
}

/**
 * Resolves the bundled platform-specific binary for `namePrefix` (e.g. `dream`, `dream-lsp`)
 * inside `binDir`: prefers `<namePrefix>-<platform>-<arch>[.exe]`, falling back to the generic
 * `<namePrefix>[.exe]`, and marks whichever is found executable (best-effort - a failure here
 * surfaces as a real error on the subsequent invocation instead). Returns `null` if neither exists.
 * Single source of truth for the platform/arch/extension-suffix packaging convention shared by
 * every bundled binary this extension launches (the compiler CLI, for both the shell-quoted "run
 * a command" use and the bare-path DAP use, and the language server), which previously
 * hand-duplicated this exact resolution three times.
 */
function resolveBundledBinary(binDir: string, namePrefix: string): string | null {
    const platform = process.platform;
    const arch = process.arch;
    const ext = platform === 'win32' ? '.exe' : '';

    const specificBinPath = path.join(binDir, `${namePrefix}-${platform}-${arch}${ext}`);
    const genericBinPath = path.join(binDir, `${namePrefix}${ext}`);

    const binPath = fs.existsSync(specificBinPath)
        ? specificBinPath
        : fs.existsSync(genericBinPath)
            ? genericBinPath
            : null;
    if (!binPath) {
        return null;
    }

    try {
        fs.chmodSync(binPath, '755');
    } catch {
        // Best-effort; if this fails the subsequent invocation will surface the real error.
    }
    return binPath;
}

/**
 * Resolves a shell command prefix for invoking the bundled `dream` compiler CLI binary.
 * Returns `null` (and shows an error message) if no bundled binary is found for this
 * platform/arch, rather than falling back to building it from source.
 */
function resolveDreamCliCommand(context: vscode.ExtensionContext): string | null {
    const binPath = resolveBundledBinary(path.join(context.extensionPath, 'bin'), 'dream');
    if (!binPath) {
        const platform = process.platform;
        const arch = process.arch;
        const ext = platform === 'win32' ? '.exe' : '';
        vscode.window.showErrorMessage(
            `Dream: no bundled compiler binary found for ${platform}-${arch} (expected "dream-${platform}-${arch}${ext}" or "dream${ext}" in the extension's bin/ folder).`
        );
        return null;
    }
    return quotePath(binPath);
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

function dreamConfig(): vscode.WorkspaceConfiguration {
    return vscode.workspace.getConfiguration('dream');
}

function readBuildSettings(): DreamBuildSettings {
    const cfg = dreamConfig();
    return {
        buildMode: (cfg.get<string>('buildMode') as BuildMode) || 'debug',
        optimizeLevel: (cfg.get<string>('optimizeLevel') as OptimizeLevel) || 'default',
        runtimeTarget: (cfg.get<string>('runtimeTarget') as RuntimeTarget) || 'native'
    };
}

async function updateBuildSetting(
    key: 'buildMode' | 'optimizeLevel' | 'runtimeTarget',
    value: string
): Promise<void> {
    await dreamConfig().update(key, value, vscode.ConfigurationTarget.Workspace);
}

/**
 * Builds CLI flag args from the current Dream settings (before the subcommand / file path).
 * Example: `['--release', '-Os', '--runtime', '--web']`.
 */
function buildDreamCliArgs(settings: DreamBuildSettings = readBuildSettings()): string[] {
    const args: string[] = [];
    if (settings.buildMode === 'release') {
        args.push('--release');
    }
    if (settings.optimizeLevel !== 'default') {
        args.push(`-O${settings.optimizeLevel}`);
    }
    if (settings.runtimeTarget === 'web') {
        args.push('--runtime', '--web');
    } else if (settings.runtimeTarget === 'node') {
        args.push('--runtime', '--node');
    }
    return args;
}

/** Resolves buildMode/optimize from a launch config, falling back to workspace settings. */
function profileFromLaunchConfig(config: vscode.DebugConfiguration): {
    buildMode: BuildMode;
    optimizeLevel: OptimizeLevel;
} {
    const settings = readBuildSettings();
    const buildMode = (config.buildMode as BuildMode | undefined) || settings.buildMode;
    const optimizeLevel =
        (config.optimizeLevel as OptimizeLevel | undefined) || settings.optimizeLevel;
    return { buildMode, optimizeLevel };
}

/** CLI flags for native run / debug-adapter (no --runtime). */
function nativeCliFlagsFromProfile(config: vscode.DebugConfiguration): string[] {
    const { buildMode, optimizeLevel } = profileFromLaunchConfig(config);
    return buildDreamCliArgs({
        buildMode,
        optimizeLevel,
        runtimeTarget: 'native'
    });
}

function formatCliArgs(args: string[]): string {
    return args.length === 0 ? '' : `${args.join(' ')} `;
}

/** Runs `dream [flags] run <file>` (or compile-only for web/node) in the Dream terminal. */
function runProgramInTerminal(
    context: vscode.ExtensionContext,
    filePath: string,
    settings: DreamBuildSettings
): void {
    const dreamCmd = resolveDreamCliCommand(context);
    if (!dreamCmd) {
        return;
    }
    const flagArgs = buildDreamCliArgs(settings);

    if (!runTerminal || runTerminal.exitStatus !== undefined) {
        runTerminal = vscode.window.createTerminal('Dream');
    }
    runTerminal.show();

    if (settings.runtimeTarget === 'native') {
        const flags = formatCliArgs(flagArgs);
        runTerminal.sendText(`${dreamCmd} ${flags}run ${quotePath(filePath)}`);
    } else {
        const flags = formatCliArgs(flagArgs);
        runTerminal.sendText(`${dreamCmd} ${flags}${quotePath(filePath)}`);
        const targetLabel = settings.runtimeTarget === 'web' ? 'browser' : 'Node';
        vscode.window.showInformationMessage(
            `Dream: compiled with ${targetLabel} runtime (use the generated *.${settings.runtimeTarget}.runtime.js host).`
        );
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
            runProgramInTerminal(context, editor.document.uri.fsPath, readBuildSettings());
        })
    );
}

function registerDebugFileCommand(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
        vscode.commands.registerCommand('dream.debugFile', async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document.languageId !== 'dream') {
                vscode.window.showWarningMessage('Open a .dream file to debug it.');
                return;
            }

            await saveActiveDreamFile(editor);

            const settings = readBuildSettings();
            if (settings.runtimeTarget !== 'native') {
                vscode.window.showWarningMessage(
                    'Dream: debugging requires the native runtime target (status bar).'
                );
                return;
            }

            const filePath = editor.document.uri.fsPath;
            const modeLabel = settings.buildMode === 'release' ? 'Release' : 'Debug';
            await vscode.debug.startDebugging(
                vscode.workspace.getWorkspaceFolder(editor.document.uri),
                {
                    type: 'dream',
                    request: 'launch',
                    name: `Dream: Debug (${modeLabel})`,
                    program: filePath,
                    buildMode: settings.buildMode,
                    optimizeLevel: settings.optimizeLevel,
                    stopOnEntry: false
                }
            );
        })
    );
}

/**
 * Resolves the path to the bundled `dream` CLI binary (without shell quoting), or `null` if none is
 * found. Mirrors `resolveDreamCliCommand` but returns a bare path suitable for a
 * `DebugAdapterExecutable`, which invokes the program directly (no shell).
 */
function resolveDreamBinaryPath(context: vscode.ExtensionContext): string | null {
    return resolveBundledBinary(path.join(context.extensionPath, 'bin'), 'dream');
}

/**
 * Wires the `dream` debug type to the CLI's `debug-adapter` subcommand: a `DebugConfigurationProvider`
 * supplies a zero-config launch for the active file (F5 with no launch.json), and a
 * `DebugAdapterDescriptorFactory` spawns the bundled `dream` binary as the DAP server over stdio.
 */
function registerDebugAdapter(context: vscode.ExtensionContext): void {
    const provider: vscode.DebugConfigurationProvider = {
        resolveDebugConfiguration(_folder, config) {
            const settings = readBuildSettings();

            // Zero-config: if launched with no configuration, debug the active .dream file.
            if (!config.type && !config.request && !config.name) {
                const editor = vscode.window.activeTextEditor;
                if (editor && editor.document.languageId === 'dream') {
                    config.type = 'dream';
                    config.request = 'launch';
                    config.name =
                        settings.buildMode === 'release'
                            ? 'Dream: Debug (Release)'
                            : 'Dream: Debug (Debug)';
                    config.program = editor.document.uri.fsPath;
                    config.buildMode = settings.buildMode;
                    config.optimizeLevel = settings.optimizeLevel;
                    config.stopOnEntry = false;
                }
            }

            if (!config.program) {
                vscode.window.showErrorMessage('Dream: no "program" set for the debug session.');
                return undefined;
            }

            if (!config.buildMode) {
                config.buildMode = settings.buildMode;
            }
            if (!config.optimizeLevel) {
                config.optimizeLevel = settings.optimizeLevel;
            }

            // Run Without Debugging / Ctrl+F5 / Run profiles: terminal, not DAP.
            if (config.noDebug) {
                const runtimeTarget = settings.runtimeTarget;
                runProgramInTerminal(context, config.program as string, {
                    buildMode: (config.buildMode as BuildMode) || settings.buildMode,
                    optimizeLevel:
                        (config.optimizeLevel as OptimizeLevel) || settings.optimizeLevel,
                    runtimeTarget
                });
                return undefined;
            }

            if (settings.runtimeTarget !== 'native') {
                vscode.window.showWarningMessage(
                    'Dream: DAP debugging uses the native wasmtime host; switch runtime target to Native or use a Run profile for web/node.'
                );
            }

            return config;
        }
    };
    context.subscriptions.push(vscode.debug.registerDebugConfigurationProvider('dream', provider));

    const factory: vscode.DebugAdapterDescriptorFactory = {
        createDebugAdapterDescriptor(session) {
            const binPath = resolveDreamBinaryPath(context);
            if (!binPath) {
                vscode.window.showErrorMessage(
                    'Dream: no bundled compiler binary found; cannot start the debugger.'
                );
                return undefined;
            }
            const program = session.configuration.program as string;
            const flags = nativeCliFlagsFromProfile(session.configuration);
            const args = [...flags, 'debug-adapter', program];
            const options: vscode.DebugAdapterExecutableOptions = {};
            if (typeof session.configuration.cwd === 'string' && session.configuration.cwd) {
                options.cwd = session.configuration.cwd;
            }
            if (
                session.configuration.env &&
                typeof session.configuration.env === 'object'
            ) {
                options.env = session.configuration.env as { [key: string]: string };
            }
            return new vscode.DebugAdapterExecutable(binPath, args, options);
        }
    };
    context.subscriptions.push(vscode.debug.registerDebugAdapterDescriptorFactory('dream', factory));
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
            // Runtime target does not affect WAT text; compile with build/opt only.
            const compileFlags = formatCliArgs(
                buildDreamCliArgs({
                    ...readBuildSettings(),
                    runtimeTarget: 'native'
                })
            );

            const command = `${dreamCmd} ${compileFlags}${quotePath(filePath)}`;
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

async function pickBuildMode(): Promise<void> {
    const current = readBuildSettings().buildMode;
    const picked = await vscode.window.showQuickPick(
        [
            {
                label: 'Debug',
                description: current === 'debug' ? '(current)' : undefined,
                detail: 'Instrumented allocator; no wasm-opt by default',
                value: 'debug' as BuildMode
            },
            {
                label: 'Release',
                description: current === 'release' ? '(current)' : undefined,
                detail: 'Trimmed allocator + wasm-opt (default -Os)',
                value: 'release' as BuildMode
            }
        ],
        { title: 'Dream: Build Mode', placeHolder: 'Select build mode' }
    );
    if (picked) {
        await updateBuildSetting('buildMode', picked.value);
    }
}

async function pickOptimizeLevel(): Promise<void> {
    const current = readBuildSettings().optimizeLevel;
    const items: Array<{
        label: string;
        description?: string;
        detail: string;
        value: OptimizeLevel;
    }> = [
        {
            label: 'Default',
            description: current === 'default' ? '(current)' : undefined,
            detail: 'Mode default (none in Debug; -Os in Release)',
            value: 'default'
        },
        { label: 'O0', detail: 'wasm-opt -O0', value: '0' },
        { label: 'O1', detail: 'wasm-opt -O1', value: '1' },
        { label: 'O2', detail: 'wasm-opt -O2', value: '2' },
        { label: 'O3', detail: 'wasm-opt -O3', value: '3' },
        { label: 'O4', detail: 'wasm-opt -O4', value: '4' },
        { label: 'Os', detail: 'wasm-opt -Os (size)', value: 's' },
        { label: 'Oz', detail: 'wasm-opt -Oz (aggressive size)', value: 'z' }
    ];
    for (const item of items) {
        if (item.value === current && item.value !== 'default') {
            item.description = '(current)';
        }
    }
    const picked = await vscode.window.showQuickPick(items, {
        title: 'Dream: Optimize Level',
        placeHolder: 'Select wasm-opt level'
    });
    if (picked) {
        await updateBuildSetting('optimizeLevel', picked.value);
    }
}

async function pickRuntimeTarget(): Promise<void> {
    const current = readBuildSettings().runtimeTarget;
    const picked = await vscode.window.showQuickPick(
        [
            {
                label: 'Native',
                description: current === 'native' ? '(current)' : undefined,
                detail: 'Run with wasmtime (dream run)',
                value: 'native' as RuntimeTarget
            },
            {
                label: 'Web',
                description: current === 'web' ? '(current)' : undefined,
                detail: 'Emit browser *.web.runtime.js (--runtime --web)',
                value: 'web' as RuntimeTarget
            },
            {
                label: 'Node',
                description: current === 'node' ? '(current)' : undefined,
                detail: 'Emit Node ≥ 18 *.node.runtime.js (--runtime --node)',
                value: 'node' as RuntimeTarget
            }
        ],
        { title: 'Dream: Runtime Target', placeHolder: 'Select runtime target' }
    );
    if (picked) {
        await updateBuildSetting('runtimeTarget', picked.value);
    }
}

function registerBuildModeCommands(context: vscode.ExtensionContext): void {
    context.subscriptions.push(
        vscode.commands.registerCommand('dream.setBuildMode', () => pickBuildMode()),
        vscode.commands.registerCommand('dream.setOptimizeLevel', () => pickOptimizeLevel()),
        vscode.commands.registerCommand('dream.setRuntimeTarget', () => pickRuntimeTarget())
    );
}

function optimizeStatusLabel(level: OptimizeLevel): string {
    if (level === 'default') {
        return 'Opt: Default';
    }
    return `Opt: O${level}`;
}

function registerStatusBar(context: vscode.ExtensionContext): void {
    const buildItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    buildItem.command = 'dream.setBuildMode';
    buildItem.tooltip = 'Dream build mode (Debug / Release)';

    const optItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 99);
    optItem.command = 'dream.setOptimizeLevel';
    optItem.tooltip = 'Dream wasm-opt level';

    const targetItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 98);
    targetItem.command = 'dream.setRuntimeTarget';
    targetItem.tooltip = 'Dream runtime target (Native / Web / Node)';

    context.subscriptions.push(buildItem, optItem, targetItem);

    const refresh = () => {
        const editor = vscode.window.activeTextEditor;
        const isDream = editor?.document.languageId === 'dream';
        if (!isDream) {
            buildItem.hide();
            optItem.hide();
            targetItem.hide();
            return;
        }
        const settings = readBuildSettings();
        buildItem.text =
            settings.buildMode === 'release' ? '$(rocket) Dream: Release' : '$(bug) Dream: Debug';
        optItem.text = `$(dashboard) ${optimizeStatusLabel(settings.optimizeLevel)}`;
        const targetLabel =
            settings.runtimeTarget === 'native'
                ? 'Native'
                : settings.runtimeTarget === 'web'
                    ? 'Web'
                    : 'Node';
        targetItem.text = `$(globe) Target: ${targetLabel}`;
        buildItem.show();
        optItem.show();
        targetItem.show();
    };

    refresh();
    context.subscriptions.push(
        vscode.window.onDidChangeActiveTextEditor(() => refresh()),
        vscode.workspace.onDidChangeConfiguration((e) => {
            if (
                e.affectsConfiguration('dream.buildMode') ||
                e.affectsConfiguration('dream.optimizeLevel') ||
                e.affectsConfiguration('dream.runtimeTarget')
            ) {
                refresh();
            }
        })
    );
}

export async function activate(context: vscode.ExtensionContext) {
    const outputChannel = vscode.window.createOutputChannel('Dream Language Server');
    outputChannel.appendLine('Activating Dream extension...');

    compilerOutputChannel = vscode.window.createOutputChannel('Dream Compiler');
    context.subscriptions.push(compilerOutputChannel);

    registerRunFileCommand(context);
    registerDebugFileCommand(context);
    registerDebugAdapter(context);
    registerShowWatCommand(context);
    registerBuildModeCommands(context);
    registerStatusBar(context);

    // Check for a bundled platform-specific binary (e.g. dream-lsp-darwin-arm64).
    const binPath = resolveBundledBinary(path.join(__dirname, '..', 'bin'), 'dream-lsp');

    let serverOptions: ServerOptions;

    if (binPath) {
        outputChannel.appendLine(`Found bundled binary at ${binPath}`);
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
            const msg =
                'Dream LSP failed to start: "cargo" is not available in your PATH, and no bundled binary was found.';
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
        vscode.window.showErrorMessage(
            `Dream LSP failed to start. Check the 'Dream Language Server' output channel for details.`
        );
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
