# Repository Agent Guidance

## Where to work right now (read this first)

- **Repo:** `Hmbown/CodeWhale`. This repo lives on multiple devices, so work in
  whichever local checkout you have — keep paths here device-agnostic and always
  **confirm with `git branch --show-current` before editing.**
- **Active branch:** start from live truth. Confirm the current fix/integration
  branch from the latest handoff/objective file and `git branch --show-current`;
  recent work has landed on `main` through small PRs rather than a long-lived
  `codex/...` integration branch, so verify a named integration branch still
  exists before relying on it.
- **Workspace version:** read it from `Cargo.toml` (`[workspace.package]
  version`); it advances per release lane, so treat that file as the source of
  truth over any memorized number. Bump versions deliberately, keeping a bump to
  its own commit.
- **Milestone guidepost:** use the current release milestone named in the active
  handoff and list it live, e.g.
  `gh issue list --repo Hmbown/CodeWhale --milestone "<current milestone>" --state open`.
- **Default branch is `main`.** Committing directly to `main` is fine for
  release-lane work — keep each commit to one reviewable concern with a real
  body. A fresh `codex/...` branch or worktree is still the right call for an
  isolated or risky change, opened as a PR when that reads better for review.
- **Always run before pushing a change:** `cargo fmt`, then the targeted tests
  for the area (`cargo test -p codewhale-tui --bin codewhale-tui --locked <filter>`,
  `cargo test -p codewhale-config`, `cargo test -p codewhale-protocol`, …). Full
  gate: `cargo test --workspace`. Release build:
  `cargo build --release -p codewhale-cli -p codewhale-tui`.
- **Known suite papercuts (pre-existing, not regressions):**
  `run_verifiers_background_*` is flaky under full-suite parallelism but passes
  in isolation. Attribute it to the known flake, not to your change. (The old
  `config_command_allow_shell_*` failures on machines with
  `default_mode = "yolo"` were fixed by pinning the command-test app to
  Agent mode.)

## Continuous agent work conventions

- One concern per commit; write a real commit body. Keep unrelated changes in
  separate commits.
- Commit as **WIP** unless you have actually verified the behavior (built the
  binary, ran the test, reproduced the fix). Stating "fixed" without evidence is
  worse than an honest WIP.
- Build only on the surfaces that exist today (removed machinery stays gone):
  the model-facing sub-agent surface is **`agent` only** — the
  `agent_open`/`agent_eval`/`agent_close`/`delegate_to_agent` variants,
  capacity/coherence/runtime-tag systems, lifecycle tools, and runtime prompt/tag
  injection were all removed. `constitution.md` is the sole base prompt.
- Configurable sub-agent depth stays. Add a new limit only when it's clearly
  needed, and explain why.
- The sub-agent **TUI freeze reported in older handoffs is resolved** by the
  v0.8.61 cutover (cap-20, persist-debounce, AgentProgress redraw throttle,
  ListSubAgents coalescing, input-pump-off-render-thread). The leading
  "blocking I/O starves the worker pool" theory was measured and **disproven**
  (`git rev-parse` ~10ms, 18-core machine). Treat the freeze as closed and spend
  effort elsewhere rather than on a speculative `spawn_blocking` fix.

## CodeWhale Stewardship

- Treat community contributors as partners. Good-faith PRs, issue reports,
  repros, logs, reviews, and verification comments are maintainer evidence,
  not queue noise.
- Keep gates warm and dry-run unless Hunter explicitly approves enforcement.
  Gate copy should guide contributors clearly and respectfully.
- Credit every harvested PR, issue report, or comment that materially shaped a
  fix. Preserve authorship when possible; otherwise use mappable GitHub
  noreply `Co-authored-by` trailers from `.github/AUTHOR_MAP`.
- CodeWhale started as a DeepSeek-only harness; it's now about building the
  greatest possible coding harness with the help of an open-source community.
  Keep CodeWhale branding and every model/provider first-class — none
  privileged. When retiring legacy names like `deepseek-tui`, keep it clear that
  every model and provider stays fully supported.
- Review PRs from code, tests, linked issues, comments, and check results — let
  those, rather than the title or labels alone, drive every merge, close,
  harvest, or defer decision on community work.
- Respect concurrent work in the tree — leave unrelated edits by other people or
  agents intact.

## Release PR Integration

- Use scratch integration branches when triaging a crowded release queue. A
  branch such as `scratch/vX.Y.Z-pr-train-YYYYMMDD` may merge or cherry-pick
  many PR heads to expose conflicts, missing tests, duplicate work, and hidden
  coupling quickly.
- Treat scratch branches as evidence, not as the artifact to ship. Land work by
  harvesting the safe resolved hunks or commits back into the release branch in
  narrow, reviewable commits — keep tags, releases, and fast-forwards off the
  scratch train.
- Prefer direct GitHub merge only when the PR is clean against the real landing
  branch, has acceptable checks, and does not cross trust-boundary surfaces. A
  PR that is clean against `main` can still conflict with a release branch; test
  against the actual release head before calling it merge-ready.
- For already approved PRs, start with a scratch merge against the release
  branch, then decide between direct merge, cherry-pick with conflict
  resolution, or credited harvest. Maintainer approval is a priority signal,
  not permission to skip review or tests.
- When harvesting, preserve or add machine-readable credit: keep the original
  author where possible, add `Co-authored-by` using `.github/AUTHOR_MAP` or
  GitHub numeric noreply identity, and include `Harvested from PR #N by
  @handle` in the commit body so the auto-close workflow can close the PR with
  credit after it reaches `main`. Merge a PR whose commit carries that line
  with rebase or a merge commit so the body survives intact — a squash can
  rewrite it, drop the `Harvested from PR` line, and silently lose both the
  machine-readable credit and the auto-close.
- Keep `Co-authored-by` trailers to human contributors —
  `scripts/check-coauthor-trailers.py` rejects bot/tool ones (Claude, codex,
  cursor, `noreply@anthropic.com`) on harvest commits. Also refresh the manual
  credit surfaces that do not auto-populate from trailers: `docs/CONTRIBUTORS.md`
  and `CHANGELOG.md`.
- Close or update issues and PRs only after verifying the landed commit on the
  relevant branch. If the release branch already contains equivalent behavior,
  leave a clear note linking the commit and describing any remaining delta.
- For the active release queue, start from the current GitHub release milestone
  named in the active handoff
  (`gh issue list --repo Hmbown/CodeWhale --milestone "<current milestone>"`) and
  refresh state before acting. Older per-version triage docs under `docs/` are
  historical reference only.
