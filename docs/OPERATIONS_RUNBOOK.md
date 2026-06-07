# codewhale Operations Runbook

This runbook covers practical debugging and incident response for the local CLI/TUI runtime.

## Quick Triage

1. Confirm binary + config:
   - `cargo run -- --version`
   - `cat ~/.codewhale/config.toml` (or inspect configured profile)
2. Enable verbose logs:
   - `RUST_LOG=deepseek_cli=debug cargo run`
   - For HTTP retries/reconnects: `RUST_LOG=deepseek_cli::client=debug cargo run`
3. Capture current state:
   - `ls ~/.codewhale/sessions`
   - `ls ~/.codewhale/sessions/checkpoints`
   - `ls ~/.codewhale/tasks`

## Incident: Turn Hangs or Stream Stops

Symptoms:
- TUI remains in loading state
- partial assistant output with no completion

Checks:
1. Inspect retry/health logs (`deepseek_cli::client`)
2. Verify endpoint connectivity:
   - `curl -sS https://api.deepseek.com/beta/models -H "Authorization: Bearer $DEEPSEEK_API_KEY"`
3. Confirm no local sandbox/permission deadlock in tool output

Actions:
1. If a foreground shell command is running, press `Ctrl+B` and choose whether to background it or cancel the current turn.
2. If the command was started in the background, ask the assistant to cancel it with `exec_shell_cancel` and the returned task id.
3. Use `Esc` or `Ctrl+C` to interrupt the current turn when you want to stop the request itself.
4. Retry prompt; if still failing, restart TUI.
5. On restart, verify the previous queued/in-flight runtime turn is shown as interrupted rather than left in a running state.

## Incident: Network Outage / Offline Behavior

Expected behavior:
- New prompts are queued while offline mode is active
- Queue state persists to `~/.codewhale/sessions/checkpoints/offline_queue.json`

Checks:
1. Open queue in TUI: `/queue list`
2. Confirm persisted queue file exists and updates timestamp

Actions:
1. Restore connectivity
2. Re-send queued entries (from `/queue edit <n>` + Enter, or normal input flow)
3. Ensure queue file clears when queue is empty

## Incident: Crash Recovery Needed

Expected behavior:
- Checkpoint stored at `~/.codewhale/sessions/checkpoints/latest.json`
- Startup begins a fresh session unless `--resume`/`--continue` is supplied

Actions:
1. Resume prior work explicitly via `codewhale --resume <id>` or `Ctrl+R` in TUI
2. If checkpoint inspection is needed, inspect `latest.json` for schema mismatch/details
3. If schema is newer than binary supports, upgrade binary or remove stale checkpoint

## Incident: Persistent State Schema Errors

Symptoms:
- Errors like `schema vX is newer than supported vY`

Affected stores:
- sessions (`~/.codewhale/sessions/*.json`)
- runtime thread/turn/item records
- tasks (`~/.codewhale/tasks/tasks/*.json`)

Actions:
1. Confirm binary version and migration expectations
2. Back up the state directory before editing
3. Either:
   - run with a newer compatible binary, or
   - archive incompatible records and regenerate state

## Incident: MCP/Tool Execution Failures

Checks:
1. Validate `~/.codewhale/mcp.json` schema and server command paths
2. Confirm server process can start manually
3. Check sandbox denials in TUI history / logs

Actions:
1. Retry with required approvals (or YOLO only when appropriate)
2. Temporarily disable failing MCP server and isolate issue
3. Re-enable after verification with `/mcp` diagnostics

## Post-Incident Checklist

1. Preserve logs and relevant state files
2. Record trigger, impact, and mitigation
3. Add or update regression tests (retry/recovery/schema)
4. Update this runbook and architecture docs if behavior changed
