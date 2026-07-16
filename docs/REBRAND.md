# Rebrand: DeepSeek TUI → Codewhale

Starting with **v0.8.41**, this project ships under a new name: `codewhale`.

This document explains what changed, what didn't, and how to migrate. None of the
DeepSeek provider integration changed — only the local CLI / TUI brand.

## TL;DR

```bash
# 1. Uninstall the old wrapper or binaries.
npm uninstall -g deepseek-tui      # or:
cargo uninstall deepseek-tui-cli 2>/dev/null || true
cargo uninstall deepseek-tui 2>/dev/null || true
                                    # legacy Homebrew installs may use:
                                    # brew upgrade deepseek-tui

# 2. Install under the new name.
npm install -g codewhale            # or:
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
                                    # legacy Homebrew installs may still use
                                    # brew install deepseek-tui until the tap
                                    # formula is renamed.

# 3. Run with the new command.
codewhale doctor
codewhale
```

Your existing `~/.deepseek/config.toml`, `~/.deepseek/sessions/`,
`~/.deepseek/skills/`, `~/.deepseek/tasks/`, and `~/.deepseek/mcp.json` are
not deleted. New Codewhale installs prefer `~/.codewhale/`, and legacy
`~/.deepseek/` state remains a read fallback while you migrate. Existing
`DEEPSEEK_*` environment variables continue to work.

## What got renamed

| Surface | Before | After |
|---|---|---|
| CLI dispatcher binary | `deepseek` | `codewhale` |
| TUI runtime binary | `deepseek-tui` | `codewhale-tui` |
| npm wrapper package | `deepseek-tui` | `codewhale` |
| Crates.io crates | `deepseek-tui-cli` / `deepseek-tui` / `deepseek-*` | `codewhale-cli` / `codewhale-tui` / `codewhale-*` |
| Release assets | `deepseek-<platform>` / `deepseek-tui-<platform>` | `codewhale-<platform>` / `codew-<platform>` / `codewhale-tui-<platform>` |
| Checksum manifest | `deepseek-artifacts-sha256.txt` | `codewhale-artifacts-sha256.txt` |

## What changed for local state

New installs write product-owned state under `~/.codewhale/`. Existing
`~/.deepseek/` config, sessions, skills, tasks, MCP config, memory, and notes
remain readable as legacy fallbacks while you migrate. Codewhale never deletes
the legacy directory automatically.

## What did NOT change

Anything that targets the DeepSeek provider API stays exactly as it was:

- **Environment variables**: `DEEPSEEK_API_KEY`, `DEEPSEEK_BASE_URL`,
  `DEEPSEEK_MODEL`, `DEEPSEEK_PROVIDER`, `DEEPSEEK_PROFILE`, `DEEPSEEK_YOLO`,
  `DEEPSEEK_LOG_LEVEL`, plus the existing `DEEPSEEK_TUI_*` runtime knobs
  (`DEEPSEEK_TUI_BIN`, `DEEPSEEK_TUI_RELEASE_BASE_URL`, etc.). They're kept
  for backward compatibility; renaming them would break every shell rc on
  the planet.
- **Model IDs**: `deepseek-v4-pro`, `deepseek-v4-flash`, and the legacy
  aliases `deepseek-chat` and `deepseek-reasoner`.
- **Hosts**: `api.deepseek.com` (global) and `api.deepseeki.com` (China
  fallback).
- **GitHub repository URL**: `https://github.com/Hmbown/CodeWhale`.
  The old `Hmbown/DeepSeek-TUI` URL redirects there during the transition.
- **Homebrew tap and formula** (`Hmbown/homebrew-deepseek-tui`): still uses
  the legacy formula name for existing installs. Treat it as compatibility-only
  until the tap is renamed; new install docs prefer `codewhale` npm, Cargo,
  Docker, or direct downloads.
- **Docker image**: `ghcr.io/hmbown/codewhale`.

## Deprecation shims (removed in v0.9.0)

To keep existing shell aliases, scripts, and CI working through the rename,
v0.8.41 and later v0.8.x releases shipped **deprecation shims**:

- A `deepseek` binary that prints a one-line warning to stderr and forwards
  argv to `codewhale`.
- A `deepseek-tui` binary that does the same for `codewhale-tui`.
- The legacy `deepseek-tui` npm package is deprecated and no longer receives
  new releases. Install the `codewhale` npm package instead.

These binary shims are removed in **v0.9.0**. DeepSeek provider support, model
IDs, `DEEPSEEK_*` environment variables, and legacy `~/.deepseek/` state
fallbacks remain supported.

## Migrating in practice

### npm

```bash
npm uninstall -g deepseek-tui
npm install -g codewhale
```

### Cargo

```bash
cargo uninstall deepseek-tui-cli 2>/dev/null || true
cargo uninstall deepseek-tui 2>/dev/null || true
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

Or in a checkout:

```bash
cargo install --path crates/cli --locked --force
cargo install --path crates/tui --locked --force
```

### Legacy `deepseek update`

Current v0.8.x compatibility binaries recognize when they are running under a
legacy `deepseek` or `deepseek-tui` filename. In that case, `deepseek update`
or `deepseek-tui update` downloads the canonical Codewhale release assets and
installs them beside the legacy binary as `codewhale` and `codewhale-tui` when
the install directory is writable.

If that update path cannot write to the install directory, use the npm, Cargo,
Homebrew, or manual reinstall commands above. The legacy npm package
`deepseek-tui` remains deprecated and is not republished; npm users should move
to `npm install -g codewhale`.

### Homebrew

**Current state (v0.9.x):** The tap formula still uses the legacy
`deepseek-tui` name for compatibility. Existing users keep running
`brew upgrade deepseek-tui`. The formula installs the same current-release
`codewhale` / `codew` / `codewhale-tui` binaries.

**Target state:** A `codewhale` formula in a renamed tap
(`Hmbown/codewhale` or the existing `Hmbown/deepseek-tui` tap with an
added `codewhale` formula alias). The legacy `deepseek-tui` formula
remains installable as a compatibility-only alias.

**Rollout steps:**

1. **Audit the formula Ruby file** — confirm it already installs
   `codewhale` / `codewhale-tui` binaries and only the formula *name* is
   legacy.
2. **Add a `codewhale` formula** to the tap that is identical to or
   aliases the existing `deepseek-tui` formula.
3. **Update website and docs** — show `brew install codewhale` as the
   primary Homebrew path, mark `brew install deepseek-tui` as legacy
   compatibility.
4. **One release of overlap** — ship at least one release with both
   `codewhale` and `deepseek-tui` formulas available so existing
   crontabs/scripts can migrate.
5. **Deprecation notice** — add a `caveat` in the legacy formula
   directing users to `brew uninstall deepseek-tui && brew install codewhale`.
6. **Eventually remove** the `deepseek-tui` formula after a deprecation
   window (e.g., two minor releases).

Until the formula rename ships, new installs should prefer npm, Cargo,
Docker, or direct downloads.

### Manual / GitHub Releases

`v0.8.41` through `v0.8.x` Releases attached the canonical `codewhale-*` /
`codewhale-tui-*` assets (plus `codew-*` from v0.8.66 onward) and
compatibility-only `deepseek-*` / `deepseek-tui-*` shim assets. Starting in
v0.9.0, Releases attach only the canonical `codewhale-*` / `codew-*` /
`codewhale-tui-*` assets and the `codewhale-artifacts-sha256.txt` checksum
manifest. Install or update through `codewhale` before moving to v0.9.0.

### Sessions, skills, and manual workspaces

Renaming the binary does not require starting over:

- **Config**: on first launch, Codewhale copies `~/.deepseek/config.toml` to
  `~/.codewhale/config.toml` if the Codewhale file does not already exist.
  It never overwrites a newer Codewhale config. You can inspect the active path
  with `codewhale doctor`.
- **Sessions and tasks**: managed state is read from `~/.codewhale/...` when
  present, with `~/.deepseek/...` used as the legacy fallback when only the old
  directory exists. Existing saved sessions still appear in `codewhale sessions`
  and the TUI resume picker.
- **Skills**: Codewhale discovers workspace skills first, then global skills,
  including both `~/.codewhale/skills` and legacy `~/.deepseek/skills`. Existing
  skill directories with `SKILL.md` do not need to be rewritten.
- **MCP config**: the default path is `~/.codewhale/mcp.json`. If that file is
  absent, Codewhale still reads legacy `~/.deepseek/mcp.json`. To use a custom
  MCP config file, set `mcp_config_path` in `config.toml` or
  `DEEPSEEK_MCP_CONFIG`.
- **Manual binary installs**: keep the dispatcher and TUI binaries as siblings
  on your `PATH`: `codewhale`, `codew`, and `codewhale-tui`. On Windows, the
  recommended user-local location is `%LOCALAPPDATA%\Programs\CodeWhale\bin`.
  On Unix-like systems, any user-writable `PATH` directory is fine as long as
  all three binaries are present.
- **Specified work directories**: running `codewhale` from a project directory,
  or launching it with a specific workspace path, does not move project files.
  Codewhale reads `<workspace>/.codewhale/config.toml` first and falls back to
  legacy `<workspace>/.deepseek/config.toml` when the new path is absent.

If both `~/.codewhale/...` and `~/.deepseek/...` copies exist, the Codewhale
path wins. Keep the legacy directory until you have confirmed `codewhale
doctor`, `codewhale sessions`, and your expected skills all show the same state.

## Why the name change

Codewhale is a shorter, terminal-friendlier handle for the same terminal
coding agent and the longer-term product direction: an agentic terminal for
open source and open-weight coding models, with DeepSeek — the provider the
project started with — remaining first-class alongside every other provider. The project name,
command names, package names, release assets, Docker image, and CNB mirror move
to Codewhale; the official DeepSeek provider, model IDs, env vars, and
`~/.deepseek/` config surface remain first-class.

## Reporting issues with the rename

If your install broke during the migration, please open an issue at
<https://github.com/Hmbown/CodeWhale/issues> and include:

- The output of `codewhale --version` (or `deepseek --version` if you're
  still on the shim).
- Which install path you used (npm, cargo, brew, manual).
- The exact command you ran and the full error output.

We'll prioritize migration regressions.
