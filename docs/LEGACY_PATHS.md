# Legacy `.deepseek/` compatibility paths — audit & migration status (#3068)

Codewhale was renamed from DeepSeek-TUI. To avoid breaking existing installs, the runtime reads
state from the new `~/.codewhale/` location but **falls back** to the legacy `~/.deepseek/` location,
and always **writes** to `~/.codewhale/`. This doc audits each legacy reference and records a
keep / deprecate / remove decision so the migration is auditable.

## The canonical resolver (use this for new code)

State-dir resolution is consolidated in `crates/config/src/lib.rs`:

| Symbol | Line | Purpose |
|---|---|---|
| `CODEWHALE_APP_DIR = ".codewhale"` | 3428 | canonical app dir |
| `LEGACY_APP_DIR = ".deepseek"` | 3431 | legacy app dir (fallback only) |
| `codewhale_home()` | 3437 | `~/.codewhale` |
| `legacy_deepseek_home()` | 3451 | `~/.deepseek` (legacy) |
| `resolve_state_dir(subdir)` | 3469 | **read** path: `~/.codewhale/<subdir>`, falling back to `~/.deepseek/<subdir>` when only the legacy dir exists |
| `ensure_state_dir(subdir)` | 3484 | **write** path: always creates under `~/.codewhale/<subdir>` |

Migration contract: read-with-fallback, write-to-new. This preserves the v0.8.44 migration for
users who still have `~/.deepseek/` while steering all new writes to `~/.codewhale/`.

## Per-path decisions

**Decision for all legacy references below: keep-as-fallback.** Removing the `.deepseek` fallback
would strand users who upgraded in place and never re-ran onboarding. Revisit only after a release
that actively migrates `~/.deepseek/` → `~/.codewhale/` on first run and a deprecation window.

| Reference | Routed through `resolve_state_dir`? | Decision |
|---|---|---|
| `config::resolve_state_dir` / `ensure_state_dir` | n/a (the resolver itself) | keep — canonical |
| `crates/tui/src/skills/mod.rs` (`~/.deepseek/skills`) | no — hardcoded | keep-as-fallback; route through resolver in a follow-up refactor |
| `crates/tui/src/prompts.rs` (`LEGACY_HANDOFF_RELATIVE_PATH = ".deepseek/handoff.md"`) | no — explicit legacy const | keep — explicit legacy handoff fallback |
| `crates/tui/src/workspace_trust.rs` | no — hardcoded | keep-as-fallback; follow-up |
| `crates/tui/src/session_manager.rs` | no — hardcoded | keep-as-fallback; follow-up |
| `crates/tui/src/skill_state.rs` | no — hardcoded | keep-as-fallback; follow-up |
| `crates/tui/src/tools/skill.rs` | no — hardcoded | keep-as-fallback; follow-up |
| `crates/tui/src/snapshot/mod.rs` | no — hardcoded | keep-as-fallback; follow-up |
| `crates/tui/src/workspace_discovery.rs` | no — hardcoded | keep-as-fallback; follow-up |

## Follow-up (separate, non-doc change — out of scope for #3068)

The optional consolidation the issue mentions — routing the hardcoded sites above through
`resolve_state_dir`/`ensure_state_dir` instead of joining `.deepseek`/`.codewhale` by hand — is a
small refactor that should land as its own PR with tests asserting read-fallback + write-to-new for
each migrated site. It is intentionally kept out of this audit so the documentation can land safely
on its own.
