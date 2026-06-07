# v0.8.51 Release Handoff — 2026-06-02

## Workspace

```
/Volumes/VIXinSSD/codewhale
Branch: codex/v0.8.51-arcee-provider (12 commits ahead of origin/main)
```

## What's Already Landed (committed, 12 commits on branch)

| Commit | What |
|--------|------|
| `e54a0a500` | feat(provider): add direct Arcee support |
| `99da87ca1` | fix(cli): wire arcee provider auth |
| `8eca75763` | test(tui): cover arcee provider picker entry |
| `06612495f` | chore(release): prep v0.8.51 — version bump, CHANGELOG |
| `fd69f4c80` | fix(tui): strip DEC private mode CSI fragments (#2592) |
| `5249723e1` | fix(engine): recover from turn panics (#2583, #1269) |
| `478bae451` | fix(tui): find deeply nested files via @/Ctrl+P (#2488) |
| `e95f759cd` | fix(tui): command-palette scroll visibility (#2590) |
| `cccc5ed55` | fix(shell): .NET/NuGet + Windows env (#1857) |
| `7aa73fad5` | fix(config): warn on misplaced shell/sandbox keys (#2589) |
| `a7d482067` | fix(clippy): clear -D warnings (#2599) |
| `79d78878b` | test(mcp): deterministic SSE reconnect (#2597) |
| `f886f28ac` | test(tui): update walk-depth test for new default depth |

**Claude branch `origin/claude/busy-mayer-b66rA` is identical to our HEAD** — same commit `f886f28ac`. Nothing to merge; already up to date.

## What's Applied in Working Tree (NOT committed)

4 files modified (88 insertions, 25 deletions):

| File | Change | Credits |
|------|--------|---------|
| `crates/tui/src/tui/diff_render.rs` | `wrap_text` preserves leading whitespace; fixes extra-space bug in PR #2591; adds 2 regression tests | @zlh124 (PR #2591, fix version from working tree) |
| `crates/tui/src/schema_migration.rs` | `#[allow(dead_code)]` on `pub mod registry` | @reidliu41 (PR #2601) |
| `crates/tui/src/prompts/base.md` | Tool desc: prefer `gh --json` CLI for GitHub triage | — |
| `crates/tui/src/prompts/base.txt` | Same prompt update for text variant | — |

## Ready to Apply (patch saved, NOT yet applied)

The model persistence patch is at `/tmp/model_persist.patch` (240 lines, 2 files):

- `crates/tui/src/commands/core.rs`: `/model` command remembers per-provider model selection + persist warning
- `crates/tui/src/settings.rs`: new `set_provider_model_selection()` and `persist_provider_model_selection()` methods

**Apply it:**
```bash
cd /Volumes/VIXinSSD/codewhale
git apply /tmp/model_persist.patch
```

This is small, self-contained, and directly improves UX for the new Arcee provider (model choice remembered across restarts). No dependency on the deferred image-attachment work.

## Deferred (in stash, do NOT apply for v0.8.51)

The stash (`stash@{0}`) contains:

- **Image attachment** (#2584/#2587): `ContentBlock::ImageUrl`, multimodal chat requests, base64 encoding, + exhaustive match arms across 15 files. Deferred by Hmbown — changes the request shape, needs multimodal endpoint testing.
- **GitHub structured route** (`fetch_url.rs`): new feature — routes GitHub issue/PR URLs through `gh` CLI. Too broad for v0.8.51.
- **Config custom model changes** (`commands/config.rs`): `normalize_custom_model_id` etc. Need review.

To view: `git stash show -p stash@{0}`

## What Remains

```bash
cd /Volumes/VIXinSSD/codewhale

# 1. Apply the model persistence patch
git apply /tmp/model_persist.patch

# 2. Commit all working-tree changes as one harvest commit
git add -A
git commit -m "harvest(v0.8.51): diff-render whitespace fix + schema dead_code + model persistence + prompt updates

- fix(diff-render): preserve leading whitespace in patch content lines
  Credit: @zlh124 (PR #2591), with extra-space bug fixed.
- fix(tui): allow unused schema migration registry
  Credit: @reidliu41 (PR #2601).
- feat(tui): persist per-provider model selection from /model command
- docs(prompts): prefer gh --json CLI for GitHub triage in agent instructions"

# 3. Run release gates
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test -p codewhale-tui -- --test-threads=4

# 4. If gates pass, rebuild
cargo install --path crates/tui --locked --force
codewhale --version
codewhale-tui --version

# 5. Final checks
git diff origin/main --stat
gh issue view 2600 --repo Hmbown/CodeWhale  # re-read release checklist
```

## Release Checklist Status (issue #2600)

- [x] Arcee provider landed + tested
- [x] Cycle/checkpoint-restart system removed
- [x] Auto-compaction percentage/model-aware
- [x] Provider/gateway HTTP error sanitization
- [x] TUI fixes (blue dot, sidebar scroll, tooltip)
- [x] CHANGELOG + version bump
- [x] Clippy clean (#2599)
- [x] MCP SSE test deterministic (#2597)
- [ ] Full `cargo test --workspace` green — codewhale-tui validated; 1 environment-only Landlock test may fail on macOS
- [ ] `npm test` in `npm/codewhale`
- [ ] Harvest commits applied + re-gated
- [ ] Merge branch to `main`
- [ ] Tag `v0.8.51`, push tag
- [ ] Publish GitHub release + `npm publish`

## Contributors to Credit

| Contributor | Contribution | PR/Issue |
|-------------|-------------|----------|
| @zlh124 (jayzhu) | diff-render whitespace preservation | #2591 |
| @reidliu41 (Reid) | schema migration dead_code allow | #2601 |
| @xyuai | Image attachment root cause + initial PR | #2587, #2584 |
| @IcedOranges | Image attachment bug report | #2584 |
| @idling11 (Hanmiao Li) | Sidebar resize feature request | #2602 |
| @gordonlu (Gordon) | Engine death recovery | #2585 |
| @cyq1017 | File picker depth fix draft | #2593 |

## Risks / Notes

1. **Working tree was stashed** — the image attachment feature and GitHub structured route are deferred for v0.8.52+. The model persistence patch is the only remaining piece worth landing.
2. **DeepSeek naming**: The branch and committed code use "CodeWhale" naming throughout. Do not imply DeepSeek is deprecated.
3. **The `origin/claude/busy-mayer-b66rA` branch is identical to HEAD** — the Claude Code session in #2600 claimed "+8 commits" but those are the same commits already on this branch. Verify with `git rev-parse HEAD origin/claude/busy-mayer-b66rA`.
4. **Landlock test**: `sandbox::tests::test_parity_linux_landlock_available` will fail on macOS (no Landlock LSM). This is environment-only, not a regression. On CI Linux runners it passes.
5. **Cross-platform artifacts**: The release workflow builds macOS + Windows + NSIS installer on tag push. Not buildable locally on macOS alone.

---

Generated by deepseek-v4-pro in CodeWhale v0.8.51 pre-release triage.
Next session: read this file, apply `/tmp/model_persist.patch`, run gates, commit, and prepare the merge.
