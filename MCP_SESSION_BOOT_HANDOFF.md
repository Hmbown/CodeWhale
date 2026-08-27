# MCP + plugin session-boot surface

Branch: `grok/v0912-mcp-session-boot-surface-20260827`

Plugin discovery and every enabled MCP server boot as a **set on the
session**, not a toast per name. Slack is one server in that set. The first
turn must not sit on `working · 22s · 0 steps` while optional servers
handshake sequentially.

## Session-boot contract

Owner: `crates/tui/src/tui/session_boot.rs`. Tests use several fake servers
(`alpha`, `beta`, `gamma`, `docs`) — never a Slack special-case.

### Zero servers

- Activity strip: no MCP chip.
- Receipt: no rows (unless plugins report invalid/duplicate/needs-setup).
- Empty session page looks like a session page, not an MCP manager.

### One server

- Booting: `MCP · 1 connecting · alpha` (name when it fits).
- Settled connected: receipt may collapse to `MCP · 1 connected`.
- Settled failed: one row `alpha · failed · /mcp retry alpha`.
- Settled needs login: one row `alpha · needs login · /mcp login alpha`.
- Settled disabled: `alpha · disabled`.

### N servers

- Booting: `MCP · 4 connecting` plus named chips when width allows
  (`alpha · beta · gamma · docs`). Narrow width sheds names, keeps the count.
- Settled mixed: compact `MCP · 3 connected` plus one row per failed / needs
  login / disabled, capped at six receipt rows with `+N more · /mcp`.
- Plugin line (only when the registry is not quiet):
  `Plugins · 12 loaded · 1 invalid · 2 duplicate`.

### Persistence

Failures remain on the session page (activity chip + receipt) until retry
succeeds. They are `Event::McpSessionBoot`, not `Event::Status` toasts.
Never tell users `/mcp auth`. Next actions are `/mcp retry <name>`,
`/mcp login <name>`, and `/mcp doctor`.

### Motion

Reduced/Still: keep the text state. No decorative spin on the receipt.
The activity-band phase marker already follows `MotionPolicy`.

## Engine

- `spawn_engine` → `Engine::run` starts `start_mcp_session_boot` immediately.
- Enabled servers connect **concurrently** (`McpPool::connect_all`, JoinSet,
  semaphore of 8). Recreated from stranded `96bc9e79c`; not merged from the
  giant `mcp-lifecycle-ui` tree.
- The connect task does **not** occupy the engine mailbox. Optional servers
  never block `mcp_tools`: while `mcp_boot_in_flight`, the first LLM call
  snapshots currently-ready tools. Catalog refreshes on a later turn
  (KV-cache prefix re-pin reason: `mcp-session-boot`).
- `/mcp retry <name>` retries one transport without dropping siblings
  (`Op::RetryMcpServer`). Recreated from the small `0933e231c` slice.

## UI

- Activity strip (`phase_strip`): MCP/plugin chip beside the live pulse.
- Compact receipt (`frame.rs` slot above the activity band): 0–6 rows from
  the auxiliary budget, like the background-work chip.
- Extensions MCP rows show connecting / login / retry without a second
  global reload.

## Worktree note

The requested SSD worktree path became unwritable (`Operation not permitted`
on `/Volumes/VIXinSSD/CW`). Implementation continued in a writable clone:

`/Users/hunterbown/codewhale-worktrees/cw-v0912-mcp-session-boot-surface-20260827`

based at the same `origin/main` (`018d32811`).
