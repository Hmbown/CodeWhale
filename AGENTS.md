# Repository Agent Guidance

## How to establish current context

- **Repo:** `Hmbown/CodeWhale`. This repo lives on multiple devices, so do
  **not** hard-code a device-specific checkout path here. Work in whichever
  local checkout you have.
- Derive live context from source-of-truth commands, not from this guidance
  file: current branch from `git branch --show-current`, workspace version from
  `Cargo.toml`, remote from `git remote -v`, and milestone scope from the
  user's task or live GitHub queries.
- If a handoff, issue, PR, or user request names a branch or milestone, verify
  it with local `git` and `gh` before editing.
- Do not bump versions opportunistically; version bumps, tags, release
  artifacts, publishing, and GitHub Releases require Hunter's explicit approval.
- **Default branch is `main`.** Never commit directly to `main`. For normal
  issue work, branch from current `origin/main` as `codex/issue-N-short-slug`
  and open a PR back to `main`. Use an active integration branch only when the
  user or issue explicitly names one.
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
- Default to one issue per branch and one reviewable PR per issue. A PR may
  close multiple issues only when the implementation is genuinely inseparable;
  otherwise keep issues independently mergeable.
- Before starting an issue, refresh it from GitHub, read linked issues/PRs, and
  identify the likely file set. If another live branch/PR is touching the same
  files or behavior, coordinate instead of racing.
- It is fine to run multiple agents/issues at the same time when their file
  sets and behavior surfaces do not conflict. Keep each branch isolated, and use
  scratch integration branches only to discover conflicts across a batch.
- If you claim an issue through GitHub, add or respect `agent-in-progress`;
  remove stale claims only after checking activity. Use `needs-human` for
  blocked credentials/product decisions rather than leaving the issue ambiguous.
- Every issue PR should include `Closes #N` or `Fixes #N`, target `main`, list
  verification commands, and call out any nearby parallel PRs or integration
  risks.
- If the current branch already contains equivalent work for an issue, do not
  reimplement it. Comment with the commit/PR evidence and prepare the closure
  path once the work reaches `main`.
- Preserve public behavior, data migrations, tests, and contributor credit; do
  not preserve a confusing file shape just because it already exists. If a file
  or module is easier and safer to remake cleanly than to surgically edit, remake
  it and migrate callers/tests to the clearer contract.
- Use **Fleet** as the user-facing term for grouped agents and durable worker
  configuration. Do not introduce a parallel non-Fleet API or docs vocabulary.
  Preferred nouns are `FleetProfile`, `FleetRole`, `FleetSlot`,
  `FleetLoadout`, and provider-agnostic Fleet model classes such as `strong`,
  `balanced`, and `fast`.
- Treat provider-prefixed model strings such as `deepseek-ai/...`,
  `deepseek/...`, `anthropic/...`, `openai/...`, and `qwen/...` as provider wire
  IDs or namespace hints only inside a provider-scoped offering. They are not
  global proof of canonical model ownership or a reason to switch providers.
- Do not force every provider into DeepSeek-style token pricing. Route displays
  may need token `PricingSku`, subscription/quota `UsageMeter`, account credits,
  local resource/not-applicable state, or explicit unknown/stale state. Codex or
  ChatGPT OAuth-style routes should show usage/quota when available, not fake
  per-token pricing.
- Do not silently change provider, model, Fleet model class, or reasoning mode
  by interpreting raw prompt text. Route changes must come from explicit user
  choice, saved config, Fleet role/slot/loadout policy, hard capability
  requirements, fallback policy, or an explicit user-enabled `auto` router with
  visible/auditable resolution.
- Use one normal user-facing automatic mode: Fleet/model loadout `auto`. It may
  resolve the whole loadout: provider, model class, model, reasoning policy,
  tool/structured/long-context requirements, and fallback policy. If an
  implementation needs route-level reasoning auto, expose it as an advanced
  explicit `reasoning_policy = "auto"` field inside the resolved loadout, not as
  a second same-looking user-facing auto knob. If an LLM/router model is used to
  make that decision, the user must configure it explicitly and CodeWhale must
  record the router, inputs, decision source, and effective route.
- When a milestone requires untangling a heavily-coupled monolith, prefer a new
  focused module/layer with a clear API, then migrate callers into it. Do not
  keep adding provider/model/security/business exceptions to a stale match table
  just because it is the shortest patch. New code is acceptable when it makes
  provider facts, model facts, and route-resolution joins easier to reason
  about than an in-place mega-refactor.
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

- Use scratch integration branches when triaging a crowded release queue or
  parallel issue train. A branch such as
  `scratch/v0.8.66-pr-train-YYYYMMDD` or
  `scratch/issues-3389-3402-YYYYMMDD` may merge or cherry-pick many PR heads to
  expose conflicts, missing tests, duplicate work, and hidden coupling quickly.
- Treat scratch branches as evidence, not as the artifact to ship. Do not tag,
  release, fast-forward a release branch, or open the scratch branch as the
  primary shipping PR. Harvest the safe resolved hunks or commits back into
  narrow issue branches or merge the already-reviewable PRs in an order that
  keeps `main` green.
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
- For release queue work, start from the live GitHub milestone named by the
  user, issue, PR, or current handoff and refresh state before acting. Older
  per-version triage docs under `docs/` are historical reference only.
