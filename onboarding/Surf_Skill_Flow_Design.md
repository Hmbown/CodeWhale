# Surf — Skill Flow Design 🏄

**Version:** 2.0  
**Date:** 2026-07-21  
**Status:** Draft

---

## Overview

Surf is a self‑contained suite for managing an isolated CodeWhale testbed. It provides both **deterministic** (no LLM) and **LLM‑enhanced** entry points.

**The metaphor:** The CodeWhale repo moves like a wave. Surf is the practice of riding it—not fighting it.

---

## Core Principles

| Principle | Description |
|---|---|
| **Deterministic by default** | The core flow works without LLM, network (except git pull), or external APIs. |
| **Self‑identifying** | The testbed is marked by `.surf-config` at its root. |
| **Fork‑aware** | The config stores the repo URL and branch. |
| **Dirty‑tree safety** | Never auto‑pulls over uncommitted changes. |
| **Receipt‑based** | Every run produces a JSON receipt for auditing. |
| **LLM optional** | The LLM is only used for summaries and guidance, never for execution. |

---

## Entry Points

| Entry Point | What It Does | LLM? |
|---|---|---|
| **`/surf`** | Orchestrator (surf.sh) | ❌ No |
| **`/surf setup`** | Clone and init (catch-wave.sh) | ❌ No |
| **`$surf`** | Skill with optional LLM summary | ✅ Yes (optional) |
| **`$surf --summary`** | Skill with forced summary | ✅ Yes |

---

## Environment State Machine

The orchestrator (`surf.sh`) uses `check-wave.sh` to determine the state of the current directory.

| State | Condition | Action |
|---|---|---|
| **empty-or-no-git** | No `.git` directory | Prompt: run `/surf setup` |
| **testbed (clean)** | `.git` + `.surf-config` + clean worktree | Run `ride-wave.sh` |
| **testbed (dirty)** | `.git` + `.surf-config` + dirty worktree | Stop with warning |
| **unknown-repo** | `.git` but no `.surf-config` | Stop with guidance |

---

## File Structure

```
.codewhale/
├── commands/
│   ├── surf.md                  # /surf  (orchestrator)
│   └── surf-setup.md            # /surf setup  (clone/init)
├── skills/
│   └── surf/
│       ├── SKILL.md             # $surf  (LLM-enhanced)
│       ├── SKILL_FLOW_DESIGN.md # This document
│       └── scripts/
│           ├── surf.sh          # MAIN ORCHESTRATOR
│           ├── check-wave.sh
│           ├── catch-wave.sh
│           └── ride-wave.sh
└── config.toml                  # not touched
```

---

## Configuration File: `.surf-config`

This file is created by `/surf setup` and sits at the root of the testbed.

```bash
# Surf configuration
REPO_URL=https://github.com/Hmbown/CodeWhale.git
BRANCH=main
ONBOARDING_INIT=true
```

| Field | Purpose |
|---|---|
| `REPO_URL` | The Git repository URL (fork or upstream) |
| `BRANCH` | The branch to track |
| `ONBOARDING_INIT` | Marker that this is a valid testbed |

---

## Deterministic Scripts

### `surf.sh` — Orchestrator

Called by `/surf`. It checks the environment, decides the action, and calls the appropriate script.

- If `STATUS=empty-or-no-git` → tells user to run `/surf setup`
- If `STATUS=testbed` + `DIRTY=false` → calls `ride-wave.sh`
- If `STATUS=testbed` + `DIRTY=true` → stops with warning
- If `STATUS=unknown-repo` → stops with guidance

### `check-wave.sh` — Environment Check

Reports the state of the current directory. Outputs:

```text
STATUS=testbed
MESSAGE=Testbed detected
DIRTY=false
```

### `catch-wave.sh` — Setup

Called by `/surf setup`. It prompts for the repo URL and branch, clones the repository, and creates `.surf-config`.

### `ride-wave.sh` — Update & Verify

Called by `surf.sh` when the testbed is clean. It:

1. Loads `.surf-config`
2. Checks that the worktree is clean
3. Switches to the configured branch if needed
4. Pulls the latest changes (`git pull --ff-only`)
5. Runs `cargo fmt --check`
6. Runs `cargo clippy -- -D warnings`
7. Runs `cargo test --workspace`
8. Extracts the latest entry from `CHANGELOG.md`
9. Writes a receipt to `receipts/latest_receipt.json`

---

## Receipt Format

```json
{
  "timestamp": "2026-07-21T10:00:00Z",
  "repo": "https://github.com/JayBeest/CodeWhale.git",
  "branch": "my-feature",
  "commit": "abc123",
  "status": "success",
  "message": "All checks passed."
}
```

---

## User Flow

### First‑time setup

```text
$ mkdir codewhale-testbed
$ cd codewhale-testbed
$ codewhale /surf setup

Enter repository URL (default: https://github.com/Hmbown/CodeWhale.git): https://github.com/JayBeest/CodeWhale.git
Enter branch (default: main): my-feature

Cloning...
Testbed initialized. Config written to .surf-config.
```

### Daily use

```text
$ cd codewhale-testbed
$ codewhale /surf

🌊 Checking the wave...
🌊 Wave is clean. Riding...
📦 Pulling latest...
...
✅ Surf complete.
📄 Receipt written: receipts/latest_receipt.json
```

### If dirty

```text
$ /surf

🌊 Checking the wave...
⚠️  The wave is choppy. Uncommitted changes detected.
📋 Clean up or stash changes before riding.
```

---

## Design Decisions

| Decision | Rationale |
|---|---|
| **Bash scripts for deterministic core** | Reliable, fast, no dependencies, easy to debug |
| **`.surf-config` as config + marker** | Simple, fork‑aware, human‑editable |
| **Receipts stored in testbed** | Persistent, self‑contained |
| **Commands for deterministic flow** | TUI‑integrated, no LLM required |
| **Skills for LLM enhancement** | Optional, additive, non‑blocking |
| **`/surf` as the main command** | Short, memorable, fits the metaphor |

---

## Future Extensions

| Extension | Description |
|---|---|
| **`/surf inspect`** | View the latest receipt without re‑running |
| **`/surf diff`** | Show what changed since last sync |
| **`$surf --suggest`** | LLM suggests next steps based on receipt |
| **Integration with constitutional testbed** | Reuse Surf for #4032-like reproduction |

---

_All of it. Fine._ 🏄🐋