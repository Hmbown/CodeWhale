import * as vscode from "vscode";
import type { RuntimeState, ThreadSummary } from "./runtime";

export class RuntimeStatusView implements vscode.WebviewViewProvider {
  public static readonly viewType = "codewhale.runtimeStatus";

  private view?: vscode.WebviewView;
  private state: RuntimeState = {
    kind: "offline",
    baseUrl: "http://127.0.0.1:7878",
    detail: "Runtime has not been checked yet.",
  };
  private threads: ThreadSummary[] = [];
  private threadsDetail = "Connect to the runtime to load recent threads.";

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = { enableScripts: true };
    view.webview.onDidReceiveMessage((message: { command?: string }) => {
      if (message.command === "check") {
        void vscode.commands.executeCommand("codewhale.checkRuntime");
      } else if (message.command === "start") {
        void vscode.commands.executeCommand("codewhale.startRuntime");
      } else if (message.command === "terminal") {
        void vscode.commands.executeCommand("codewhale.openTerminal");
      } else if (message.command === "threads") {
        void vscode.commands.executeCommand("codewhale.refreshAgentView");
      }
    });
    this.render();
  }

  update(state: RuntimeState): void {
    this.state = state;
    this.render();
  }

  updateThreads(threads: ThreadSummary[], detail: string): void {
    this.threads = threads;
    this.threadsDetail = detail;
    this.render();
  }

  private render(): void {
    if (!this.view) {
      return;
    }

    const badge = labelFor(this.state.kind);
    const nonce = makeNonce();
    const threadsHtml =
      this.threads.length > 0
        ? this.threads.map((thread) => renderThread(thread)).join("")
        : `<p class="detail">${escapeHtml(this.threadsDetail)}</p>`;
    this.view.webview.html = `<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}';">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <style>
    body { padding: 14px; color: var(--vscode-foreground); font-family: var(--vscode-font-family); }
    .status { margin-bottom: 12px; font-weight: 600; }
    .detail { margin: 0 0 14px; color: var(--vscode-descriptionForeground); line-height: 1.45; }
    .section-title { margin: 18px 0 8px; font-size: 11px; font-weight: 700; letter-spacing: 0; text-transform: uppercase; color: var(--vscode-descriptionForeground); }
    .thread { padding: 8px 0; border-top: 1px solid var(--vscode-sideBarSectionHeader-border, var(--vscode-panel-border)); }
    .thread-title { margin-bottom: 4px; font-weight: 600; overflow-wrap: anywhere; }
    .thread-preview { margin-bottom: 5px; color: var(--vscode-descriptionForeground); line-height: 1.35; overflow-wrap: anywhere; }
    .thread-meta { color: var(--vscode-descriptionForeground); font-size: 11px; overflow-wrap: anywhere; }
    code { color: var(--vscode-textLink-foreground); }
    button { width: 100%; margin: 4px 0; }
  </style>
</head>
<body>
  <div class="status">${escapeHtml(badge)}</div>
  <p class="detail">${escapeHtml(this.state.detail)}</p>
  <p class="detail"><code>${escapeHtml(this.state.baseUrl)}</code></p>
  <button data-command="check">Check Runtime</button>
  <button data-command="threads">Refresh Threads</button>
  <button data-command="start">Start Local Runtime</button>
  <button data-command="terminal">Open CodeWhale Terminal</button>
  <div class="section-title">Agent View</div>
  ${threadsHtml}
  <script nonce="${nonce}">
    const vscode = acquireVsCodeApi();
    for (const button of document.querySelectorAll("button[data-command]")) {
      button.addEventListener("click", () => vscode.postMessage({ command: button.dataset.command }));
    }
  </script>
</body>
</html>`;
  }
}

function renderThread(thread: ThreadSummary): string {
  const status = thread.latestTurnStatus ? ` · ${thread.latestTurnStatus}` : "";
  const archived = thread.archived ? " · archived" : "";
  const branch = thread.branch ? ` · branch ${thread.branch}` : "";
  const workspace = thread.workspace ? ` · ${thread.workspace}` : "";
  const updated = thread.updatedAt ? ` · ${formatTimestamp(thread.updatedAt)}` : "";
  return `<div class="thread">
    <div class="thread-title">${escapeHtml(thread.title)}</div>
    <div class="thread-preview">${escapeHtml(thread.preview || "No recent message.")}</div>
    <div class="thread-meta">${escapeHtml(`${thread.mode} · ${thread.model}${status}${branch}${archived}${updated}${workspace}`)}</div>
  </div>`;
}

function labelFor(kind: RuntimeState["kind"]): string {
  switch (kind) {
    case "connected":
      return "Connected";
    case "auth-required":
      return "Token Required";
    case "error":
      return "Runtime Error";
    case "offline":
      return "Offline";
  }
}

function formatTimestamp(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString();
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function makeNonce(): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let nonce = "";
  for (let index = 0; index < 32; index += 1) {
    nonce += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
  }
  return nonce;
}
