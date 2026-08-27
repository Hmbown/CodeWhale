# TUI settings / MCP recovery lane — 2026-08-27

## Branch and SHA

- Worktree: `/Volumes/VIXinSSD/CW/worktrees/cw-v0912-tui-settings-mcp-20260827`
- Branch: `grok/v0912-tui-settings-mcp-20260827`
- Base: freshly fetched `origin/main` at `a96ea6cb09cc464ea2e88f251c538c239d1fe9ad`
- Product commit: `8e15e51d43433f5e2e924611aed21ba4fa5b6709`
  `feat(tui): make settings MCP recovery first-class and clickable`

No push, PR, merge, release, or deploy was performed. The machine `codewhale`
binary was not installed or overwritten. `/Volumes/VIXinSSD/CW/cwc` was not
edited. Occupied worktrees were not reused.

`docs/CURRENT.md` is not present on this `origin/main`. `AGENTS.md` and
`docs/SETTINGS_PICKER_FRAMEWORK.md` / `docs/MCP.md` were the live contracts.

## PR #5643 overlap

Open PR #5643 (`codex/v0912-tui-polish-20260827`, `92ef9aa4d`) already contains
the MCP login copy fix in `streamable_http.rs` **and** the welcome-shine timing
fix. It is **not** in `origin/main`. This lane built on `origin/main` and did
**not** copy the welcome-shine / composer / launch-menu work.

The `/mcp auth` recovery sentence still exists on current `main`. This slice
fixes that string (required: recovery copy must name commands that exist) and
then **deepens settings/plugin management** rather than duplicating 5643's
animation work.

If both land, the `streamable_http.rs` hint change will overlap with 5643 and
should be a trivial conflict: both replace `/mcp auth` with `/mcp login`.

## User journeys that changed

1. **Settings (`F2` / `/settings`) is clickable.** Category tabs
   (General / Models / Permissions / Display / Advanced) have mouse hitboxes.
   One click switches the tab. Rows keep the existing select-then-activate
   pattern.

2. **Settings → Advanced → MCP is no longer a path-only wall.** Calm action
   rows, each with an obvious next command:
   - MCP manager → `/mcp`
   - Reconnect MCP → `/mcp reload`
   - Diagnose MCP → `/mcp validate`
   - Plugins → `/plugin`
   Enter (or the second click on a selected action row) runs that command.

3. **Extensions → MCP recovery is first-class.** Each server row's action is
   Connect / Reconnect / Re-auth / Diagnose / Enable, mapped to real commands:
   - disabled → `/mcp enable <name>`
   - not inspected → `/mcp reload` (Connect)
   - disconnected → `/mcp reload` (Reconnect)
   - 401 / unauthorized / oauth-capable stale session → `/mcp login <name>`
   - other errors or healthy connected → `/mcp validate` (Diagnose)
   There is no `/mcp auth`.

4. **Plugin invalid manifests and duplicates are diagnosable.** Registry
   diagnostics (including `manifest-invalid`, `duplicate-root`, `name-conflict`)
   appear in a Problems group. Error-bearing plugins run `/plugin validate
   <name>` instead of a mute Inspect.

5. **`/mcp` manager pager names a next action per server** (`next: Re-auth
   /mcp login github`) instead of a command-prose wall. Footer is
   `Next: Connect /mcp reload · Diagnose /mcp validate`.

6. **Stale Streamable HTTP OAuth recovery** now says `/mcp login <name>`
   (CLI remains `codewhale mcp login`). Bearer-only sessions still point at
   the configured token, not a login command.

7. **Hunyuan / Hy in model selection.** There is no native Tencent Hunyuan
   `ApiProvider` and no models.dev bundled row. The public OpenRouter model
   `tencent/hy3-preview` already exists. Search/canonical aliases now include
   `hy3`, `hunyuan`, `tencent-hunyuan`, `hunyuan-hy3`. **hy4 is not invented.**

## Files changed

- `crates/tui/src/tui/views/mod.rs` — clickable tabs; MCP/plugin action rows
- `crates/tui/src/tui/views/extensions.rs` — MCP/plugin recovery actions + Problems
- `crates/tui/src/tui/mcp_routing.rs` — per-server next actions in `/mcp` pager
- `crates/tui/src/mcp.rs` — `McpRecoveryKind` + `mcp_recovery_kind`
- `crates/tui/src/mcp/oauth.rs` — `tui_reauth_hint` / text classifier
- `crates/tui/src/mcp/streamable_http.rs` — `/mcp login` recovery copy
- `crates/tui/src/localization.rs` + 15 locale packs
- `crates/tui/src/config.rs`, `crates/config/src/lib.rs`, `crates/agent/src/lib.rs`
  — Hunyuan aliases for hy3 only
- tests next to those modules

## Tests actually run

Isolated build-dir via `scripts/dev-cache.sh`. Command:

```
cargo nextest run -p codewhale-tui --lib --locked -E 'test(/mcp_recovery_kind|tui_reauth_hints|manager_text_names_login|config_view_tabs_are_clickable|config_view_mcp_action_rows|openrouter_hunyuan|mcp_item_action_for|message_id_list_english|shipped_complete_packs_have_raw|settings_registry_types_every|config_view_includes_expected_editable|config_view_can_edit_filtered|config_view_mouse_click_selects|auth_required_login_hint_names|manager_text_shows_failed/)'
```

Result: **16 passed**, 11198 skipped. Also `cargo fmt --all -- --check` and
`git diff --check`.

Not run: full `codewhale-tui --lib` suite, clippy `-D warnings`, live TUI/PTY
at multiple widths, live MCP OAuth, live Hunyuan/OpenRouter calls (no provider
money spent).

## What remains

- Welcome shine vs launch-menu regression is still on `main`; 5643 owns that
  fix. This lane did not duplicate it.
- `/mcp` manager is still a pager, not a clickable modal. Recovery is named
  on each row; clicking the pager text does not run the command.
- Settings action rows still use select-then-activate (same as provider/model),
  not single-click-to-run. Tabs are single-click.
- No native Hunyuan provider adapter. If Tencent ships a public API distinct
  from OpenRouter `tencent/hy3-preview`, that is a later provider lane. Do not
  add hy4 until a public id exists.
- Plugin Problems rows always run `/plugin validate` (global); they do not yet
  deep-link a specific diagnostic path into a dedicated inspector.
- Locale packs have the new keys; only English was used in the focused tests
  besides the completeness parity gate.

## Safest next integration step

Review `8e15e51d4` on this branch. Do not merge over 5643's welcome-shine
files. If 5643 merges first, rebase this branch; expect a small
`streamable_http.rs` overlap on the login hint only. Then PTY-check Settings
tabs and an MCP 401 row at ~80 and ~40 columns.
