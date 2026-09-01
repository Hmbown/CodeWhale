# Environment-specific caveats

Standard build/test/run commands live in `AGENTS.md` and `CONTRIBUTING.md`.
This file records only the non-obvious quirks of particular environments, so
they do not cost context on machines that will never hit them.

## Cursor Cloud VMs

- **System build dep:** the build needs `libdbus-1-dev` (pulled in by
  `crates/secrets` for the OS keyring). It is installed by the startup update
  script; if a `cargo build` fails with a `dbus`/`pkg-config` error, that dep is
  missing.
- **`rustup default` must be set:** some tests and runtime paths spawn shells in
  temp dirs *outside* this checkout (e.g. `run_verifiers_background_*`, sub-agent
  worktrees). Those spawned shells only see the repo's `rust-toolchain.toml`
  override while inside `/workspace`, so without a global default they fail with
  "rustup could not choose a version of rustc to run". The update script runs
  `rustup default stable` to fix this.
- **Known env-specific test failures at `/workspace` (not code bugs):** because
  the checkout sits directly under `/`, two `codewhale-tui` subagent tests fail
  here — `git_repo_root_reports_attempted_paths_when_no_repo_found` (cannot
  create a temp dir in the unwritable parent `/`) and
  `create_isolated_worktree_reports_friendly_error_when_no_repo_found` (walking
  up to `/` discovers `/workspace` itself as a repo). Both pass when the repo is
  checked out under a normal, writable parent.

## Running the agent without provider API keys

Point Codewhale at any local OpenAI-compatible endpoint via the keyless
`vllm`/`ollama`/`sglang` providers:

```sh
CODEWHALE_PROVIDER=vllm VLLM_BASE_URL=http://127.0.0.1:8000/v1 VLLM_MODEL=<id> \
  codewhale exec --auto "..."
```

`codewhale exec` (add `--auto` for tool use) is the non-interactive path to
exercise the full agent loop.

## Consolidated runtime commands

The current `codewhale` binary runs the TUI in-process. Release installers copy
the same bytes to the optional `codew` short command; no sibling
`codewhale-tui` executable is required. `DEEPSEEK_TUI_BIN` remains a legacy
replay/migration setting, not a current install requirement.
