# Harness Profile Cutline

This note defines the v0.9.0 order for HarnessProfile work. The automatic
Harness Creator must not run before the profile schema, resolver, seed
profiles, and user-visible status surfaces are explicit and tested.

## Decision

For v0.9.0, CodeWhale should treat harness profiles as typed policy data first.
Automatic profile evolution is deferred until replay evidence, candidate
manifests, and promotion gates exist.

The first implementation lane stops at:

1. `HarnessPosture` enum and policy knobs.
2. `HarnessProfile` schema and registry.
3. Deterministic profile resolver.
4. Seed profiles for common model families.
5. Repo constitution overlay input.
6. Status/UX display of the resolved provider, model, profile, and repo law.

Only after those surfaces are visible and tested should CodeWhale add evidence
stores, candidate manifests, promotion gates, or an agentic Harness Creator.

## Required Seed Profiles

| Model family | Intended posture | Notes |
| --- | --- | --- |
| DeepSeek V4 Pro / Flash | cache-heavy | Preserve prefix stability and large-context continuity. |
| Xiaomi MiMo V2.5 Pro / UltraSpeed / V2.5 | cache-heavy | Similar long-context/cache posture, but route and auth remain distinct from DeepSeek. Older V2 Flash names are historical examples, not current direct-provider defaults. |
| Arcee Trinity Thinking | cache-heavy or explicit Arcee profile | Direct Arcee IDs such as `trinity-large-thinking` must not be hidden behind OpenRouter aliases. |
| Hugging Face / local / open-weight routes | lean | Prefer smaller context packs, stricter tool surfaces, and subagent-oriented decomposition. |
| Generic OpenAI-compatible gateways | standard unless matched | Do not infer provider-specific posture from a bare endpoint alone. |

Provider route, endpoint, model id, HarnessProfile, and repo constitution must be
separately visible. A profile resolver may choose a profile, but it must not
silently change provider auth, base URLs, model IDs, tool allowlists, or repo
permissions.

## Repo Constitution Boundary

`.codewhale/constitution.json` is local repo law, not another provider profile.
The resolver may read it as an input after project trust checks, but profile
selection must show both:

- the model-facing posture, such as `cache-heavy` or `lean`;
- the repo-law source, such as `.codewhale/constitution.json` or none.

## Automatic Evolution Boundary

AHE/GEPA-style profile evolution is future work. It can be referenced as
inspiration only after the text distinguishes these stages:

1. candidate proposal from recorded evidence;
2. replay/eval against a weaker or constrained student;
3. promotion-gate decision with required tests and policy checks;
4. inspectable overlay update or rollback.

No v0.9.0 harness profile should be silently promoted, mutated, or written to a
cached-main overlay by the schema/resolver/display lane.

## Smoke Evidence

Before v0.9.0 ships with HarnessProfile runtime behavior beyond schema parsing
and pure resolver checks, the acceptance matrix should record evidence for:

- DeepSeek V4 resolving to a cache-heavy profile;
- Xiaomi MiMo resolving to a cache-heavy profile without sharing DeepSeek auth;
- Arcee direct `trinity-large-thinking` resolving through the direct `arcee`
  route, not the OpenRouter `arcee-ai/trinity-large-thinking` alias;
- a generic/HF/local model resolving to a lean or standard profile;
- the TUI or runtime status surface showing provider, model, profile, and repo
  constitution separately;
- no automatic profile mutation during normal Agent or WhaleFlow runs.

For v0.9.0, pure resolver tests may satisfy the profile-selection evidence, but
status display and runtime use remain deferred until separate PRs wire those
surfaces deliberately. Release notes should still call HarnessProfile a typed
schema/resolver foundation rather than an automatic harness creator.
