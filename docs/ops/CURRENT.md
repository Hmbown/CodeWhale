# Current lane state

**This file is expected to go stale. Verify before you act on it.**

Everything here is a snapshot of a moving release lane. `AGENTS.md` holds the
durable rules; this holds the perishable state, so that a stale milestone number
never sits next to a rule and borrows its authority.

Last updated: 2026-08-02.

## Lane

- **Repo:** `Hmbown/CodeWhale`. It lives on multiple devices — work in whichever
  local checkout you have and confirm with `git branch --show-current` before
  editing.
- **Branch:** recent work lands on `main` through small PRs rather than a
  long-lived `codex/...` integration branch. Verify any named integration branch
  still exists before relying on it.
- **Workspace version:** `0.9.4` — but read it from `Cargo.toml`
  (`[workspace.package] version`), which is the source of truth over any number
  written here.
- **Milestone:** `v0.9.4`. List it live rather than trusting this line:

  ```sh
  gh issue list --repo Hmbown/CodeWhale --milestone "v0.9.4" --state open
  ```

## Known test flakes (pre-existing, not regressions)

- `run_verifiers_background_*` flakes under full-suite parallelism but passes in
  isolation. Rerun in isolation before blaming your change.

## Closed investigations — do not reopen without new evidence

- **Sub-agent TUI freeze** (reported in older handoffs) is resolved by the
  v0.8.61 cutover: cap-20, persist-debounce, AgentProgress redraw throttle,
  ListSubAgents coalescing, input-pump-off-render-thread. The leading "blocking
  I/O starves the worker pool" theory was measured and **disproven**
  (`git rev-parse` ~10ms, 18-core machine). Do not spend effort on a speculative
  `spawn_blocking` fix.
- The old `config_command_allow_shell_*` failures on machines with
  `default_mode = "yolo"` were fixed by pinning the command-test app to Agent
  mode.

## Known debt (deliberate, not a bug to fix casually)

- The workflow *history* card renders with `Locale::En` until locale is threaded
  through `ToolCell::lines_with_mode` (~30 call sites).
- The `classic` ocean treatment exists in code but persisted settings normalize
  it away; do not expand it without a product decision.
