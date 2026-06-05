# RLM Branching Roadmap

This note records the v0.8.45 design direction for RLM, DSPy, GEPA, and Model
Lab without adding runtime dependencies or changing the live agent loop.

## Branching Primitive

CodeWhale uses the same branching primitive at three scales:

1. Release tracks. Each milestone fans into named tracks. A track must stay
   independently reviewable, mergeable, and slippable. Unfinished work rolls
   forward instead of blocking the release.
2. Capability worksets. Model Lab capabilities such as Hugging Face
   (Inference, MCP, Hub), observability, evals, serving, DSPy, GEPA, and
   training infrastructure ship as opt-in worksets with their own feature
   flag, install path, license note, and telemetry posture.
3. Pareto compile branches. Optimizable modules keep candidate
   `(instructions, demos, score)` triples. Branches that violate pinned
   constitution clauses are pruned; branches that win at least one eval remain
   on the frontier until the maintainer lands or rejects them.

The maintainer chooses the frontier point. CodeWhale should not collapse
branches prematurely.

## v0.8.45

- Close the current control-plane and workbench issues before the broader
  fan-out begins: #1982, #2027, #2032, #2016, and #2034.
- Keep `AGENTS.md` and `CLAUDE.md` maintainer-local. `AGENTS.md` is ignored
  from this milestone forward.
- Land the RLM symbolic-object substrate: active prompt, session metadata,
  transcript, latest user message, and per-message refs are named objects that
  RLM can open without copying raw prompt/history text into the parent
  transcript.

## v0.8.46

- Generalize Fin into a structured-feedback verifier substrate.
- Add first replay-eval definitions harvested from existing trajectories.
- Scaffold the Repeatability Score footer slot as pending until evals populate
  it.
- Add module artifact schema v0 as Rust types only.
- Draft the "Compiled Word" constitution article.

## v0.8.47

- Promote Hugging Face as a first-class provider through Inference Providers
  and Router.
- Add deterministic RLM replay: context snapshot, seed, child model IDs, and
  temperatures.
- Route large logs and payloads to RLM workbench sessions instead of the
  parent transcript.
- Add sub-query memoization keyed by prompt, context hash, and model.
- Enforce RLM budgets at the Rust registry layer: depth, calls, wall time, and
  cost.

## v0.8.48

- Remove the legacy `deepseek` and `deepseek-tui` shim binaries.
- Finish Docker and Homebrew rename cleanup.
- Populate Repeatability Score from a small offline eval suite that ships in
  core.

## v0.9.0

- Emit per-turn `trajectory.jsonl` as the trainset substrate.
- Add `codewhale replay <turn_id>` for deterministic replay.
- Render module artifacts from the `[[ ## field ## ]]` form through a Rust
  adapter.
- Land the eval pipeline: suites, replay evals, and measurement substrate.
- Add a `/compile` command stub that explains the offline loop.

## v0.10.0

- Add opt-in Model Lab workset installers for DSPy and GEPA. The default
  install keeps zero Python dependencies.
- Build the first offline compile pipeline: Rust harvests trainsets, a Python
  sidecar runs the optimizer, and CodeWhale emits a reviewed Module JSON
  artifact.
- Add the Compile TUI panel with Pareto frontier, lineage tree, and
  Land/Reject/Revise actions.
- Land the first optimized tool-description and agent-prompt artifacts through
  PRs. Constitution clauses remain pinned outside the optimized region.
- Add whale-species module passports, for example
  `Sei: codewhale-agent-prompt.v0.10.0-gepa-1`.

## Trust Boundary

Compilation is offline. Runtime consumes reviewed JSON artifacts. Online
closed-loop optimization is out of scope because adversarial users could game a
live coding harness. Any workset can fail independently without dragging the
release, the core runtime, or other Pareto branches with it.
