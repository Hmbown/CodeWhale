# Claude Repository Guidance

Read `AGENTS.md` first. This file exists as a compatibility instruction source
for Claude-based agents working in this repository.

## Stewardship Defaults

- Treat community PRs and issues as maintainer evidence. Inspect code, tests,
  linked issues, comments, and CI before merging, harvesting, closing, or
  deferring work.
- Default to one issue per branch and one reviewable PR per issue. Branch from
  current `origin/main` as `codex/issue-N-short-slug` unless the user or issue
  explicitly names an integration branch.
- Open implementation PRs against `main` with `Closes #N` or `Fixes #N`,
  verification commands, and a short note about nearby parallel PRs or conflict
  risk.
- Multiple agents may work issues at the same time when their file sets and
  behavior surfaces do not conflict. Use scratch branches only to batch-test
  conflicts and merge order; do not ship the scratch branch itself.
- If claiming an issue through GitHub, add or respect `agent-in-progress` and
  use `needs-human` when credentials, product calls, or unavailable context are
  the blocker.
- If equivalent work is already present on the active branch, comment with
  commit/PR evidence and prepare the closure path once it reaches `main`
  instead of reimplementing it.
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
- When old code has become a pile of cross-cutting conditionals, prefer a new
  focused module/layer with a clear API and migrate callers into it. For
  provider/model work, keep provider facts, model facts, and route-resolution
  joins separate instead of adding more one-off branches to the same table.
- Do not tag, publish, create a GitHub Release, or push release artifacts
  without Hunter's explicit approval.
- Keep CodeWhale branding while preserving first-class DeepSeek model/provider
  support and legacy migration care.
- Preserve contributor credit for harvested work with authorship,
  `Co-authored-by`, `Harvested from PR #N by @handle`, and changelog/release
  notes where applicable.

## Scratch Integration Branches

- For release queues, create disposable local branches from the real landing
  branch, for example `scratch/vX.Y.Z-pr-train-YYYYMMDD`.
- Use the scratch branch to merge or cherry-pick candidate PR heads in batches
  and learn which conflicts, tests, and overlaps are real.
- Do not ship the scratch branch itself or open it as the main implementation
  PR. It may contain noisy merge commits, partial conflict resolutions, and
  unrelated PR interactions.
- After the scratch experiment, move only the safe result back to narrow issue
  branches or merge the already-reviewable PRs in a tested order. Keep each
  final commit explainable and testable.
- A PR that is clean against `main` is not necessarily clean against a release
  branch. Test mergeability against the branch that will actually receive the
  work.
- For already approved PRs, treat approval as a strong priority signal. Still
  inspect diffs, comments, check results, and release-branch conflicts before
  landing.

## Current Context Discovery

- This repo lives on multiple devices, so do not hard-code a checkout path or
  treat this file as the source of truth for branch, version, or milestone.
- Confirm the active branch with `git branch --show-current` before editing.
  Never commit directly to `main`.
- Read the workspace version from `Cargo.toml`. Do not tag, publish, create a
  GitHub Release, push release artifacts, or merge to `main` without Hunter's
  explicit approval.
- Base release triage on the live GitHub milestone named by the user, issue,
  PR, or current handoff. Refresh state with `gh` before acting.
- Work the queue in this order: release blockers, recently approved PRs, clean
  PRs with small scope, blocked PRs with obvious fixes, dirty PRs that can be
  harvested safely, then larger architecture issues.
- Prefer batching PR conflict discovery on scratch branches, then harvesting
  reviewed, credited, tested slices back into the release branch.
- Before claiming an issue is done, verify whether the branch already contains
  equivalent work. If it does, prepare the GitHub note/closure path instead of
  reimplementing it.
- See `AGENTS.md` → "How to establish current context" for build/test commands, known
  suite papercuts, and the removed-machinery guardrails (agent-only surface,
  no lifecycle/coherence systems).
