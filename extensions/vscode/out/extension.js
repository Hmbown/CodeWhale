"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = __importStar(require("vscode"));
const runtime_1 = require("./runtime");
const status_1 = require("./status");
function activate(context) {
    const output = vscode.window.createOutputChannel("CodeWhale");
    const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    const statusView = new status_1.RuntimeStatusView();
    let autoRefreshTimer;
    let autoRefreshInFlight = false;
    status.command = "codewhale.checkRuntime";
    context.subscriptions.push(output, status);
    context.subscriptions.push(vscode.window.registerWebviewViewProvider(status_1.RuntimeStatusView.viewType, statusView));
    const refreshAgentView = async () => {
        const config = (0, runtime_1.readRuntimeConfig)();
        const threads = await (0, runtime_1.listThreadSummaries)(config);
        statusView.updateThreads(threads, "Showing recent runtime threads.");
        output.appendLine(`Loaded ${threads.length} runtime thread summaries.`);
    };
    const refreshSnapshots = async () => {
        const config = (0, runtime_1.readRuntimeConfig)();
        const snapshots = await (0, runtime_1.listSnapshots)(config);
        statusView.updateSnapshots(snapshots, "Showing recent restore points.");
        output.appendLine(`Loaded ${snapshots.length} runtime restore points.`);
    };
    const refreshAgentViewDetails = async (showWarning) => {
        try {
            await refreshAgentView();
        }
        catch (error) {
            const detail = error instanceof Error ? error.message : String(error);
            statusView.updateThreads([], "Runtime thread summaries unavailable.");
            output.appendLine(`Runtime thread summaries unavailable: ${detail}`);
            if (showWarning) {
                void vscode.window.showWarningMessage(detail);
            }
        }
        try {
            await refreshSnapshots();
        }
        catch (error) {
            const detail = error instanceof Error ? error.message : String(error);
            statusView.updateSnapshots([], detail);
            output.appendLine(`Runtime restore points unavailable: ${detail}`);
            if (showWarning) {
                void vscode.window.showWarningMessage(detail);
            }
        }
    };
    const updateStatus = (text, tooltip) => {
        status.text = text;
        status.tooltip = tooltip;
        status.show();
    };
    const checkAndRefreshRuntime = async (showSpinner, logResult) => {
        const config = (0, runtime_1.readRuntimeConfig)();
        if (showSpinner) {
            updateStatus("$(sync~spin) CodeWhale", "Checking CodeWhale runtime...");
        }
        const state = await (0, runtime_1.checkRuntime)(config);
        statusView.update(state);
        switch (state.kind) {
            case "connected":
                updateStatus("$(check) CodeWhale", state.detail);
                await refreshAgentViewDetails(false);
                break;
            case "auth-required":
                updateStatus("$(lock) CodeWhale", state.detail);
                statusView.updateThreads([], "Runtime token is required before threads can load.");
                statusView.updateSnapshots([], "Runtime token is required before restore points can load.");
                break;
            case "offline":
            case "error":
                updateStatus("$(warning) CodeWhale", state.detail);
                statusView.updateThreads([], "Connect to the runtime to load recent threads.");
                statusView.updateSnapshots([], "Connect to the runtime to load restore points.");
                break;
        }
        if (logResult) {
            output.appendLine(`${new Date().toISOString()} ${state.kind}: ${state.detail}`);
        }
        return state;
    };
    const runAutoRefresh = async () => {
        if (autoRefreshInFlight) {
            return;
        }
        autoRefreshInFlight = true;
        try {
            await checkAndRefreshRuntime(false, false);
        }
        finally {
            autoRefreshInFlight = false;
        }
    };
    const scheduleAutoRefresh = () => {
        if (autoRefreshTimer) {
            clearInterval(autoRefreshTimer);
            autoRefreshTimer = undefined;
        }
        const intervalSeconds = (0, runtime_1.readRuntimeConfig)().agentViewRefreshIntervalSeconds;
        if (intervalSeconds === 0) {
            output.appendLine("Agent View auto-refresh is disabled.");
            return;
        }
        autoRefreshTimer = setInterval(() => {
            void runAutoRefresh();
        }, intervalSeconds * 1000);
        output.appendLine(`Agent View auto-refresh scheduled every ${intervalSeconds}s.`);
    };
    updateStatus("$(terminal) CodeWhale", "Check CodeWhale runtime");
    scheduleAutoRefresh();
    context.subscriptions.push(new vscode.Disposable(() => {
        if (autoRefreshTimer) {
            clearInterval(autoRefreshTimer);
        }
    }), vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("codewhale.agentViewRefreshIntervalSeconds")) {
            scheduleAutoRefresh();
        }
    }));
    context.subscriptions.push(vscode.commands.registerCommand("codewhale.openTerminal", () => {
        const config = (0, runtime_1.readRuntimeConfig)();
        (0, runtime_1.openCodeWhaleTerminal)(config);
        output.appendLine(`Opened CodeWhale terminal using ${config.commandPath}.`);
    }));
    context.subscriptions.push(vscode.commands.registerCommand("codewhale.startRuntime", () => {
        const config = (0, runtime_1.readRuntimeConfig)();
        (0, runtime_1.startRuntimeTerminal)(config);
        const baseUrl = (0, runtime_1.runtimeBaseUrl)(config);
        updateStatus("$(sync~spin) CodeWhale", `Runtime terminal started for ${baseUrl}`);
        output.appendLine(`Started CodeWhale runtime terminal at ${baseUrl}.`);
        void vscode.window.showInformationMessage(`CodeWhale runtime starting at ${baseUrl}`);
    }));
    context.subscriptions.push(vscode.commands.registerCommand("codewhale.checkRuntime", async () => {
        return await checkAndRefreshRuntime(true, true);
    }));
    context.subscriptions.push(vscode.commands.registerCommand("codewhale.refreshAgentView", async () => {
        await refreshAgentViewDetails(true);
    }));
    context.subscriptions.push(vscode.commands.registerCommand("codewhale.refreshSnapshots", async () => {
        try {
            await refreshSnapshots();
        }
        catch (error) {
            const detail = error instanceof Error ? error.message : String(error);
            statusView.updateSnapshots([], detail);
            output.appendLine(`Runtime restore points unavailable: ${detail}`);
            void vscode.window.showWarningMessage(detail);
        }
    }));
    context.subscriptions.push(vscode.commands.registerCommand("codewhale.openRuntimeDocs", () => {
        void vscode.env.openExternal(vscode.Uri.parse("https://github.com/Hmbown/CodeWhale/blob/main/docs/RUNTIME_API.md"));
    }));
    void vscode.commands.executeCommand("codewhale.checkRuntime");
}
function deactivate() {
    // No background process is owned by the extension; runtime starts in a user-visible terminal.
}
//# sourceMappingURL=extension.js.map