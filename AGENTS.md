# Repository Agent Guidance

## Intent is the artifact (read this first)

We have crossed the threshold where **writing the code again is cheaper than
recovering the code we wrote.** Assume that is true by default and act on it.

- **The durable asset is intent** — what a change was *for*, the behavior it
  promised, the constraint it honored. The diff that expressed it is disposable.
  When you find work that is stranded, stale, or hard to reconcile, recover the
  intent and re-implement it against today's code. Do not archaeology your way
  through a merge you can retype in less time.
- **Rewriting any part of this project is always in scope**, up to and including
  the whole thing. A module, a crate, a subsystem, the TUI's 16k-line `ui.rs` —
  none of it is load-bearing by virtue of existing. If the current shape is
  fighting you, the rewrite is a legitimate first option, not a last resort.
  Argue it on merit, not on sunk cost.
- **Use git; do not be governed by it.** Branches, merge bases, and patch-ids
  are bookkeeping, not authority. A branch that is 600 commits behind is not a
  debt to be paid — it is a note describing something we once wanted. Read the
  note, decide if we still want it, then either build it fresh or close it.
  Conflict count is a signal to rewrite, not a task list.
- **A stranded lane becomes an issue, not a merge.** Default disposition for
  drifted work: open an issue that states the intent, the behavior we wanted,
  and any evidence worth keeping (repro, test, linked report); reference the
  dead branch for provenance; delete or abandon the branch. Then implement it
  from current `main` when it comes up the queue.
- **Verify before you rebuild.** The one thing that must not be lazy is the
  check for whether main *already* does it. Grep for the symbols and behavior,
  not the commit. Re-landing work that already landed is the failure mode this
  ethos creates, and it is the one you own.

### What this does not license

- **`main` stays protected and releases stay reproducible.** Rewrite freely in a
  branch or worktree; do not rewrite published history, retag a shipped release,
  or force-push a shared ref.
- **Stewardship obligations survive a rewrite.** Contributor credit, the
  `Co-authored-by` / `Harvested from PR #N` machinery, and the licensing that
  comes with community work are not artifacts of the old diff — carry them onto
  the new implementation. Re-implementing someone's contribution does not
  launder away their authorship.
- **The do-not-delete guardrail below still applies.** "Rewriting is in scope"
  is not "this looks dead to me." Verify consumers with `rg` first.
- **Don't rewrite to avoid understanding.** Rewrite because you know what the
  code should do and the current shape is in the way — not because reading it
  is tedious.

## Where to work right now

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
  injection were all removed. The constitution (`BASE_PROMPT` in
  `tui/src/prompts/text.rs`) is the sole base prompt.
- Configurable sub-agent depth stays. Add a new limit only when it's clearly
  needed, and explain why.
- **Do-not-delete guardrail** (salvaged from the 0.8.68 handoff; these were
  repeatedly misflagged as dead code and deleting them broke the build):
  `tui/src/memory.rs`, `tui/src/context_budget.rs`,
  `tui/src/model_registry.rs`, `tui/src/prompt_zones.rs`,
  `tui/src/tools/remember.rs`, and the entire `config/src/route/` directory
  are all actively imported. Verify consumers with `rg` before believing any
  dead-code audit.
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

The guidance below is for **live** work — open PRs and branches close enough to
`main` to reconcile honestly. For work that has drifted far enough that the merge
is an excavation, see "Intent is the artifact" above: capture the intent as an
issue, drop the branch, rebuild from current `main`. A useful rule of thumb — if
the conflicts are in the files the branch most wanted to change, you are
reconstructing intent anyway; do it in the editor, not the merge tool.

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

## Cursor Cloud specific instructions

Standard build/test/run commands are already documented above and in
`CONTRIBUTING.md`; this section only records the non-obvious cloud-VM caveats.

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
  checked out under a normal, writable parent. `run_verifiers_background_*` is
  the separate pre-existing flake already noted above. Everything else in
  `cargo test --workspace` passes (~6384 tests).
- **Running the agent without provider API keys:** point CodeWhale at any local
  OpenAI-compatible endpoint via the keyless `vllm`/`ollama`/`sglang` providers,
  e.g. `CODEWHALE_PROVIDER=vllm VLLM_BASE_URL=http://127.0.0.1:8000/v1
  VLLM_MODEL=<id> codewhale exec --auto "..."`. `codewhale exec` (add `--auto`
  for tool use) is the non-interactive path to exercise the full agent loop.
- **Dispatcher needs its sibling:** the `codewhale` binary shells out to a
  sibling `codewhale-tui` in the same directory (both land in `target/debug`
  after a build). If they are not co-located, set `DEEPSEEK_TUI_BIN` to the
  `codewhale-tui` path.
