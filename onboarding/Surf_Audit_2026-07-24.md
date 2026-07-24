# Surf Module — Code & Architecture Audit

**Date:** 2026-07-24
**Branch:** `wip/onboarding_suit`
**Scope:** `onboarding/` directory — design docs, bash scripts, command/skill scaffolding
**Artifacts reviewed:** `Surf_Skill_Flow_Design.md`, `Skill_Flow_Design (old).md`, `surf.md`, `surf-setup.md`, `SKILL.md`, `surf.sh`, `check-wave.sh`, `catch-wave.sh`, `ride-wave.sh`
**Codebase surfaces traced:** `crates/tui/src/commands/mod.rs`, `crates/tui/src/commands/user_registry.rs`, `crates/tui/src/commands/user_commands.rs`, `crates/tui/src/commands/groups/skills/skills.rs`, `crates/tui/src/skills/mod.rs`, `docs/architecture/command-dispatch.md`

---

## Summary

You have a well-designed **draft** with 4 working bash scripts, a design doc, and scaffolding files — but the module is **not wired to the TUI runtime** and has several critical architecture gaps. The entire `onboarding/` directory is untracked on branch `wip/onboarding_suit`. No parts of this are discoverable or executable from inside CodeWhale today.

The design thinking is solid — clean state machine, good decomposition, fork-aware, receipt-based. But the implementation rests on an `execute:` frontmatter mechanism in user commands that **does not exist** in CodeWhale's current codebase. That's the root blocker. The bash scripts work in isolation, but the bridge to the TUI is missing.

---

## What Exists (inventory)

| Artifact | State | Notes |
|---|---|---|
| `Surf_Skill_Flow_Design.md` | Complete (v2.0) | Clean state machine, good principles, clear metaphor |
| `Skill_Flow_Design (old).md` | Stale | References old "onboarding-suite" naming — should be archived or deleted |
| `surf.sh` (orchestrator) | Working | Delegates to check/ride, writes receipt, handles all 4 states |
| `check-wave.sh` | Working | Correctly identifies 4 states via `.git` + `.surf-config` — minor logic issue noted below |
| `catch-wave.sh` (setup) | Needs work | Uses `read -p` — won't work in TUI dispatch |
| `ride-wave.sh` (update & verify) | Working | Pulls, switches branches, runs fmt/clippy/test, writes receipt |
| `surf.md` (`/surf` command) | Broken | `execute:` frontmatter is silently ignored by the runtime |
| `surf-setup.md` (`/surf setup` command) | Broken | Same `execute:` problem |
| `SKILL.md` (`$surf` skill) | Stale | Uses old "onboarding-suite" name + wrong script references |

---

## Critical Gaps

### GAP 1 — `execute:` frontmatter does not exist in CodeWhale

The command `.md` files use frontmatter like:

```yaml
---
execute: .codewhale/skills/surf/scripts/surf.sh
description: "🌊 Ride the CodeWhale wave. Updates, builds, and tests the testbed."
---
```

But the metadata parser in `crates/tui/src/commands/user_registry.rs` (lines 245–262) only recognizes these frontmatter keys:

- `description`
- `argument-hint`
- `allowed-tools`
- `pausable`
- `alias` / `aliases`
- `hidden`

The `execute:` key falls into the catch-all arm `_ => {}` on line 261 and is **silently ignored**. The body text after the frontmatter closing `---` is sent as an LLM prompt via `AppAction::SendMessage` (line 508–509 in `user_registry.rs`):

```rust
// crates/tui/src/commands/user_registry.rs:508-509
let message = user_commands::apply_template(&metadata.body, args);
Some(CommandResult::action(AppAction::SendMessage(message)))
```

**This is the root blocker.** The entire Surf design hinges on a shell-script execution model that CodeWhale's user command system does not support. User commands dispatch by sending their markdown body as a prompt to the LLM, not by spawning a subprocess.

**Resolution paths:**
- (A) Add `execute:` support to the user command registry — a new feature requiring Rust changes in `user_registry.rs` (`parse_metadata`, `UserCommandMetadata`, and `try_dispatch`)
- (B) Make surf a **native built-in command** — add a group in `crates/tui/src/commands/groups/` that spawns the scripts directly
- (C) Ship it as a standalone CLI tool invoked via `codewhale exec` rather than through the TUI command namespace
- (D) Package it as a CodeWhale skill that instructs the LLM to call the scripts — but this defeats the "deterministic, no LLM" principle stated in the design doc

### GAP 2 — Discovery path mismatch

Commands and skills are discovered at these workspace-relative paths:

- **Commands:** `<workspace>/.codewhale/commands/` (see `crates/tui/src/commands/user_commands.rs:52-63`, `commands_dirs` function)
- **Skills:** `<workspace>/.codewhale/skills/` (see `crates/tui/src/skills/mod.rs:711-718`, `skill_directories_for_workspace_and_dir` function)

Your artifacts live at `onboarding/.codewhale/{commands,skills}/`. The repo root has no `.codewhale/` subdirectory (confirmed by inspection). So **neither `/surf` nor `$surf` would be found** from within the CodeWhale repo workspace. The files are two directory levels too deep.

### GAP 3 — SKILL.md is stale and references wrong names

`onboarding/.codewhale/skills/surf/SKILL.md` still uses:

| Field | SKILL.md value | Design doc expects |
|---|---|---|
| Skill name | `onboarding-suite` | `surf` |
| Triggers | `/onboarding-suite`, `/onboarding-suite --summary` | `/surf`, `$surf` |
| Script names | `check-testbed.sh`, `setup-testbed.sh`, `update-testbed.sh` | `check-wave.sh`, `catch-wave.sh`, `ride-wave.sh` |
| Marker file | `.onboarding-init` | `.surf-config` |
| Output format | `Test passed: X, failed: Y, skipped: Z` | JSON receipt in `receipts/latest_receipt.json` |

The description text also references relative directory paths that don't match the actual layout on disk. If this SKILL.md were loaded as a skill, it would inject incorrect instructions into the LLM prompt.

### GAP 4 — Skills inject text into prompts, never execute scripts

Even if SKILL.md were correct and discoverable, the skill activation path in `crates/tui/src/commands/groups/skills/skills.rs` (lines 380–383) only injects the SKILL.md body as a system instruction into the LLM prompt:

```rust
// crates/tui/src/commands/groups/skills/skills.rs:380-383
let instruction = format!(
    "You are now using a skill. Follow these instructions:\n\n# Skill: {}\n\n{}\n\n---\n\nNow respond to the user's request following the above skill instructions.",
    skill.name, skill.body
);
```

It does **not** execute any bash scripts. The `Skill` struct (defined in `crates/tui/src/skills/mod.rs:70-79`) has fields for `name`, `description`, `body`, `localized_descriptions`, and on-disk `path` — no `execute` or `script` field exists.

The LLM would need to voluntarily find and run the scripts via `exec_shell`, which contradicts the design doc's "**Deterministic by default** — The core flow works without LLM" principle.

---

## Secondary Issues

### check-wave.sh: DIRTY logic after early exit

```bash
if [ ! -d ".git" ]; then
    echo "STATUS=empty-or-no-git"
    echo "MESSAGE=No Git repository found."
    exit 0                               # exits here for empty-dir
fi

# ... (testbed/unknown-repo detection) ...

if [ -n "$(git status --porcelain)" ]; then   # runs for all non-empty states
    echo "DIRTY=true"
else
    echo "DIRTY=false"
fi
```

This works correctly because `exit 0` fires before the `git status` check for the empty-dir case. But the `DIRTY` output is appended for the `unknown-repo` state, making the output for `unknown-repo` look like:

```
STATUS=unknown-repo
MESSAGE=Git repository without .surf-config marker.
DIRTY=false
```

The design doc doesn't specify whether `DIRTY` is meaningful for `unknown-repo`. The orchestrator (`surf.sh`) only inspects `DIRTY` for the `testbed` case, so this is benign — but the output contract is looser than documented.

### ride-wave.sh: dual receipt ownership

Both `surf.sh` and `ride-wave.sh` independently write `receipts/latest_receipt.json`:

- **`surf.sh`** (line ~48-55): Writes a simplified receipt with `timestamp`, `branch`, `commit`, `status`, and a static `digest` field
- **`ride-wave.sh`** (lines ~76-85): Writes a richer receipt with `timestamp`, `repo`, `branch`, `commit`, `status`, and `message` (omits `digest`)

Because `surf.sh` runs `ride-wave.sh` first (via pipe to `tee` on line 43), then writes its own receipt after, the `surf.sh` version is what persists. The richer output from `ride-wave.sh` is lost. Pick one owner — either let `ride-wave.sh` own the receipt exclusively, or have `surf.sh` merge/enrich the ride-wave receipt.

### ride-wave.sh: fragile CHANGELOG extraction

```bash
sed -n '/## \[/,/## \[/p' CHANGELOG.md | head -n -1 | tail -n +2 | head -n 10
```

This sed range depends on exact `## [version]` formatting in `CHANGELOG.md`. If:
- The format changes (e.g., `## v0.9.0` instead of `## [0.9.0]`)
- The file uses a different heading level
- The file only has one entry (the range won't close)

…the pipeline silently produces empty output. There's no fallback or validation.

### catch-wave.sh: interactive prompts won't work in TUI

```bash
read -p "Enter repository URL (default: https://github.com/Hmbown/CodeWhale.git): " REPO_URL
read -p "Enter branch (default: main): " BRANCH
```

When dispatched through the TUI's user command path, stdin is not a terminal — it's the prompt body. These `read` calls would hang or fail. The script needs to accept arguments (`catch-wave.sh <repo-url> <branch>`) or the TUI would need to present a modal for input.

### No tests for any script

Zero test coverage. Shell scripts that perform `git clone`, `cargo test --workspace`, and `git pull --ff-only` need smoke tests at minimum — even simple assertions like `[[ -f .surf-config ]]` after a setup run, or testing the state machine transitions with a mock git repo.

### CHANGELOG.md dependency

`ride-wave.sh` assumes `CHANGELOG.md` exists and matches the expected format. On a sparse checkout or a repo that doesn't use `CHANGELOG.md`, the digest step silently degrades with a warning — but the receipt still claims `"status": "success"`.

### No dry-run or partial-verify mode

`ride-wave.sh` is all-or-nothing: `cargo fmt --check` + `cargo clippy -- -D warnings` + `cargo test --workspace`. A full workspace test suite on a large Rust project can take 10+ minutes. There's no flag to do a quick "just pull and check formatting" or "just test one crate." This limits the usefulness for rapid daily use.

### Receipt directory is CWD-relative

Both `surf.sh` and `ride-wave.sh` create `receipts/` in the current working directory, not relative to the testbed root or `.surf-config` location. If you run the script from a subdirectory, the receipt ends up in the wrong place.

---

## Design Assessment

### Pros

- **Clean state machine** — 4 states (`empty-or-no-git`, `testbed` clean, `testbed` dirty, `unknown-repo`) with deterministic transitions, easy to reason about and test
- **Fork-aware** — `.surf-config` stores repo URL + branch, no hardcoded upstream. Users can track forks or custom branches
- **Receipt-based** — every run produces traceable JSON, good for CI audit trails and automated tooling
- **Self-contained** — one `.codewhale` subdirectory holds all scripts, commands, and the skill — no scattering
- **Well-decomposed** — orchestrator (`surf.sh`) delegates to single-responsibility scripts: `check-wave.sh` for state detection, `catch-wave.sh` for setup, `ride-wave.sh` for update+verify
- **Defense in depth** — `check-wave.sh` catches dirty trees before `surf.sh` decides to ride; `ride-wave.sh` independently double-checks cleanliness before pulling
- **`set -euo pipefail`** — all scripts use it, which is correct shell hygiene and prevents silent failures
- **`--ff-only` pull** — uses fast-forward-only pulls, which prevents merge conflicts from silently appearing in the testbed
- **Good metaphor** — "Surf" / wave riding is memorable, developer-friendly, and creates a clear mental model

### Cons

- **Overloaded metaphor at the script level** — scripts named `catch-wave.sh`, `ride-wave.sh`, `check-wave.sh` are clever but lack a shared prefix like `surf-`. Grepping for "all surf scripts" requires knowing all the wave verbs
- **All-or-nothing verification** — no way to do a targeted `cargo test -p <crate>` or skip clippy for rapid iteration
- **No dry-run mode** — no flag to preview what would happen without executing. Helpful for first-time users and debugging
- **Git-only** — the `.surf-config` marker requires a `.git` directory. The design doesn't support non-git directories or other VCSs
- **Interactive prompts in setup** — `catch-wave.sh` uses `read -p`, which conflicts with TUI dispatch (see Gap above)
- **SKILL.md / design doc naming drift** — old onboarding-suite naming persists in the skill file while the design doc has moved to Surf. The `Skill_Flow_Design (old).md` file should be archived or deleted to avoid confusion
- **Duplicate receipt writes** — two scripts write to the same path with different schemas (see Secondary Issues)

---

## Codebase Context

### How user commands work today

The dispatch flow from `crates/tui/src/commands/mod.rs` (line 109, `execute` function):

1. `$skillname` → resolved as `/skill name` (line 114–133)
2. User commands checked first via `user_registry::try_dispatch` (line 148)
3. Permanent compatibility aliases (`/jihua`, `/zidong`, `/slop`, `/canzha`) (line 154–171)
4. Built-in registry lookup (line 173–175)
5. Legacy migration hints (`/set`, `/deepseek`, `/doctor`) (line 177–188)
6. Skills fallback via `groups::skills::run_skill_by_name` (line 193)

When a user command matches (step 2), `try_dispatch` in `user_registry.rs:444-509`:
- Validates the command exists
- Resets hunt state, todos, plan state
- Applies `allowed-tools` restriction if present
- Applies `pausable` flag if present
- Sends the markdown body (after frontmatter stripping) as a user message via `AppAction::SendMessage`

There is no code path that spawns a subprocess based on frontmatter. The `execute:` concept would need to be added here.

### How skills work today

Skills are discovered from these workspace directories (see `crates/tui/src/skills/mod.rs:711-718`):

1. `<workspace>/.agents/skills`
2. `<workspace>/skills`
3. `<workspace>/.opencode/skills`
4. `<workspace>/.claude/skills`
5. `<workspace>/.cursor/skills`
6. `<workspace>/.codewhale/skills`

When a skill is activated (`activate_skill` in `skills.rs:361-395`):
- The SKILL.md body is injected as a system instruction into the LLM prompt
- A system message is added to the chat history: `"Activated skill: <name>\n\n<description>"`
- `app.active_skill` is set so the next user turn includes the skill instructions
- No scripts are executed; no subprocess is spawned

### Permanent exceptions (from `docs/architecture/command-dispatch.md`)

The command dispatch system has several permanent exceptions (compatibility aliases, migration hints) that bypass the normal registry. A new built-in command for surf would be a regular addition, not an exception.

---

## Suggestions

1. **Decide on the execution model first.** The `execute:` frontmatter doesn't exist. Options:
   - **Add `execute:` support** — modify `parse_metadata` in `crates/tui/src/commands/user_registry.rs` to recognize `execute:`, add a field to `UserCommandMetadata`, and change `try_dispatch` to spawn the subprocess instead of (or in addition to) sending the body as a prompt. This is the smallest surface change but needs careful security review (path traversal, what CWD, what shell).
   - **Make surf a native built-in command** — add a `surf` group under `crates/tui/src/commands/groups/` that spawns the scripts directly. This is the cleanest integration but moves the scripts out of the markdown file and into Rust.
   - **Ship as a standalone CLI tool** — keep the bash scripts, ship them as a companion tool invoked via `codewhale exec` or a shell alias. Simplest to build, but loses TUI integration.
   - **Skill-only approach** — rewrite SKILL.md so the LLM is instructed to find and run the scripts. Simple to implement but violates the "deterministic by default" principle.

2. **Fix SKILL.md** — rename to `surf`, update triggers to `/surf` and `$surf --summary`, reference wave-named scripts and `.surf-config`

3. **Fix discovery path** — move files to `<repo-root>/.codewhale/{commands,skills}/` (or the user-global `~/.codewhale/{commands,skills}/`) so they're actually discoverable

4. **Replace `read -p` in catch-wave.sh** — accept positional args: `catch-wave.sh <repo-url> <branch>` with defaults when omitted

5. **Consolidate receipt writing** — let `ride-wave.sh` own the receipt; remove the duplicate simplified write in `surf.sh`

6. **Add `--dry-run` and `--quick` flags** to `ride-wave.sh`:
   - `--dry-run`: print what would happen without executing
   - `--quick`: skip clippy and test, only fmt-check (useful for daily "did anything break?" checks)

7. **Write smoke tests** — even simple assertions go a long way:
   - `check-wave.sh` returns correct states for a mock directory tree
   - `.surf-config` is created with correct fields after `catch-wave.sh`
   - `surf.sh` refuses to run on a dirty testbed

8. **Archive or delete** `Skill_Flow_Design (old).md` — having both old and new design docs in the same directory is confusing and risks contributors editing the wrong one

9. **Document the distribution model** — where does this live in the final product? As an installable CodeWhale skill? As a template users copy? As a built-in? The `onboarding/` directory's role in the repo needs to be explicit.

---

_Version: 1.0 — based on codebase at `wip/onboarding_suit` as of 2026-07-24_
