# CodeWhale for VS Code

Official CodeWhale extension scaffold for local development.

This first slice is intentionally small:

- open CodeWhale in an integrated terminal
- start `codewhale serve --http` in a visible terminal
- check a local runtime through `/health` and `/v1/runtime/info`
- show connection state in the status bar
- show a read-only Agent View with recent runtime thread summaries from
  `/v1/threads/summary`
- show recent read-only restore points from `/v1/snapshots`
- refresh the read-only Agent View automatically so branch/workspace metadata
  catches up while agents are working

It does not expose the full chat webview, VS Code Agent View chat/editor
integration, inline edit application, marketplace publish workflow, or
retry/undo/snapshot GUI endpoints yet.

## Local Use

```bash
npm install
npm run compile
npm run package
code --install-extension codewhale-vscode-0.8.53.vsix
```

Configure `codewhale.commandPath`, `codewhale.runtimeHost`,
`codewhale.runtimePort`, `codewhale.runtimeToken`, and
`codewhale.agentViewRefreshIntervalSeconds` from VS Code settings.
Set the refresh interval to `0` to disable automatic read-only refreshes.

Keep the runtime on `127.0.0.1` unless you deliberately front it with trusted
local networking controls.
