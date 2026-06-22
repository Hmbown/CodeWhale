# Repository Agent Guidance

## Where to work right now (read this first)

- **Repo:** `Hmbown/CodeWhale`. This repo lives on multiple devices, so do
  **not** hard-code a device-specific checkout path here — work in whichever
  local checkout you have and always **confirm with
  `git branch --show-current` before editing.**
- **Active branch:** `codex/v0.8.63-integration` (also at
  `origin/codex/v0.8.63-integration`) for the current fix/integration lane.
  If a newer handoff or objective file names a different branch, verify with
  `git branch --show-current` and follow the live branch.
- **Workspace version is `0.8.63`** in `Cargo.toml`. Do not bump versions
  opportunistically; version bumps, tags, release artifacts, publishing, and
  GitHub Releases require Hunter's explicit approval.
- **Milestone guidepost:** GitHub milestone `v0.8.63`. Check live state with
  `gh issue list --repo Hmbown/CodeWhale --milestone "v0.8.63" --state open`.
- **Default branch is `main`.** Never commit directly to `main`; work on the
  active integration branch or a fresh `codex/...` branch/worktree off it for
  an isolated change. Open a PR into `main` only when a unit of work is
  reviewable.
- **Always run before pushing a change:** `cargo fmt`, then the targeted tests
  for the area (`cargo test -p codewhale-tui --bin codewhale-tui --locked <filter>`,
  `cargo test -p codewhale-config`, `cargo test -p codewhale-protocol`, …). Full
  gate: `cargo test --workspace`. Release build:
  `cargo build --release -p codewhale-cli -p codewhale-tui`.
- **Known suite papercuts (pre-existing, not regressions):**
  `config_command_allow_shell_*` fail on machines whose `~/.codewhale/settings.toml`
  sets `default_mode = "yolo"` (the tests aren't hermetic); `run_verifiers_background_*`
  is flaky under full-suite parallelism but passes in isolation. Don't treat
  these as caused by your change.

## Continuous agent work conventions

- One concern per commit; write a real commit body. Don't squash unrelated
  changes.
- Commit as **WIP** unless you have actually verified the behavior (built the
  binary, ran the test, reproduced the fix). Stating "fixed" without evidence is
  worse than an honest WIP.
- Don't reintroduce removed machinery: the model-facing sub-agent surface is
  **`agent` only** (no `agent_open`/`agent_eval`/`agent_close`/`delegate_to_agent`
  /etc.); no capacity/coherence/runtime-tag systems; no lifecycle tools; no
  runtime prompt/tag injection. `constitution.md` is the sole base prompt.
- Configurable sub-agent depth stays. No arbitrary new limits unless clearly
  needed and explained.
- The sub-agent **TUI freeze reported in older handoffs is resolved** by the
  v0.8.61 cutover (cap-20, persist-debounce, AgentProgress redraw throttle,
  ListSubAgents coalescing, input-pump-off-render-thread). The leading
  "blocking I/O starves the worker pool" theory was measured and **disproven**
  (`git rev-parse` ~10ms, 18-core machine). Do not commit a speculative
  `spawn_blocking` fix for the freeze.

## CodeWhale Stewardship

- Treat community contributors as partners. Good-faith PRs, issue reports,
  repros, logs, reviews, and verification comments are maintainer evidence,
  not queue noise.
- Keep gates warm and dry-run unless Hunter explicitly approves enforcement.
  Gate copy should guide contributors clearly and respectfully.
- Credit every harvested PR, issue report, or comment that materially shaped a
  fix. Preserve authorship when possible; otherwise use mappable GitHub
  noreply `Co-authored-by` trailers from `.github/AUTHOR_MAP`.
- Do not tag, publish, create a GitHub Release, or push release artifacts
  without Hunter approval.
- Use CodeWhale branding while keeping DeepSeek support first-class. Retiring
  legacy `deepseek-tui` names must never read as deprecating DeepSeek models or
  provider support.
- Review PRs from code, tests, linked issues, comments, and check results.
  Never merge, close, harvest, or defer community work from title or labels
  alone.
- Respect concurrent work in the tree. Do not revert or rewrite unrelated
  edits by other people or agents.

## Release PR Integration

- Use scratch integration branches when triaging a crowded release queue. A
  branch such as `scratch/v0.8.59-pr-train-YYYYMMDD` may merge or cherry-pick
  many PR heads to expose conflicts, missing tests, duplicate work, and hidden
  coupling quickly.
- Treat scratch branches as evidence, not as the artifact to ship. Do not tag,
  release, or fast-forward a release branch from a scratch train. Harvest the
  safe resolved hunks or commits back into the release branch in narrow,
  reviewable commits.
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
  credit after it reaches `main`.
- Close or update issues and PRs only after verifying the landed commit on the
  relevant branch. If the release branch already contains equivalent behavior,
  leave a clear note linking the commit and describing any remaining delta.
- For the active release queue, start from the GitHub `v0.8.63` milestone
  (`gh issue list --repo Hmbown/CodeWhale --milestone "v0.8.63"`) and refresh
  state before acting. Older per-version triage docs under `docs/` are
  historical reference only.
