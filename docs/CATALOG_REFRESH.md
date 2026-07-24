# Catalog refresh

How CodeWhale keeps model metadata current — what already auto-updates, what
is hand-maintained, and what a scheduled catalog job should (and should not) do.

Related docs: [`MODEL_PROVIDER_AUDIT.md`](./MODEL_PROVIDER_AUDIT.md),
[`PROVIDERS.md`](./PROVIDERS.md), RFC
[`rfcs/UNIFIED_PROVIDER_LOGIN.md`](./rfcs/UNIFIED_PROVIDER_LOGIN.md).

---

## Short answer

| Question | Answer |
|---|---|
| Do users need a special model just to refresh models? | **No.** |
| Does CodeWhale auto-update the public model catalog? | **Yes, at runtime**, from [Models.dev](https://models.dev/catalog.json), ~24 h TTL. |
| Is the offline bundled seed auto-committed in CI? | **Not yet.** Live cache covers running installs; the in-repo seed is still manual / PR-driven. |
| Should an LLM rewrite catalog JSON? | **No.** Ingest is deterministic public JSON. An LLM can *review* a PR, not own the source of truth. |

---

## Layers (lowest → highest priority)

Effective precedence for model facts (context, output caps, reasoning,
pricing-ish metadata). Live wins over bundled when present.

```
(5) Legacy static completion lists (DEFAULT_* consts)
      — only if catalog has zero rows for the provider
(4) Static code tables
      crates/tui/src/models.rs
(3) Bundled offline seeds (NOT competing truth)
      crates/config/assets/models_dev.bundled.json
      crates/tui/assets/model_catalog.bundled.json
(2) Live Models.dev catalog (preferred when available)
      https://models.dev/catalog.json
      → disk cache ~/.codewhale/catalog/models-dev-catalog.json
      → 24 h TTL
(1) User / custom overrides (pinned models, custom endpoints)
(0) Special: ChatGPT/Codex OAuth roster
      ~/.codex/models_cache.json
      — bypasses Models.dev for openai-codex only
```

Key code:

| Piece | Path | Role |
|---|---|---|
| Live fetch + cache | `crates/tui/src/models_dev_live.rs` | Background refresh, TTL, atomic write, freshness status |
| Schema / parse | `crates/config/src/models_dev.rs` | Network-free Models.dev JSON shape |
| Compile + provenance | `crates/config/src/catalog.rs` | Bundled / Live / UserOverride; id normalization |
| Provider lake merge | `crates/tui/src/provider_lake.rs` | Live-over-bundled by `(provider, wire_model_id)` |
| Offline seed asset | `crates/config/assets/models_dev.bundled.json` | Compact offline fallback only (`_meta.role` says so) |
| Validation script | `scripts/catalog_models_dev.py` | Secret-free fetch/validate dry-run (#4117) |
| Script tests | `scripts/catalog_models_dev_test.py` | Offline shape/scrub checks |

---

## What already auto-updates (runtime)

When the TUI/runtime starts (and is not disabled):

1. Seed pickers from the **on-disk cache** if present (even if stale).
2. If the cache is missing or older than **24 hours**, **background-fetch**
   Models.dev (15 s timeout, explicit CodeWhale user-agent, **no credentials**).
3. On success: atomic write to
   `~/.codewhale/catalog/models-dev-catalog.json` and publish rows into
   ProviderLake as `CatalogSource::Live`.
4. On failure: keep prior cache or fall back to the **bundled** seed. Model
   selection never hard-fails because Models.dev is down.

### Manual force refresh

In the TUI:

```text
/model refresh
```

That dispatches `AppAction::RefreshModelsDevCatalog` (async; does not block
the composer). Implementation lives under
`crates/tui/src/commands/groups/core/core.rs` and
`crates/tui/src/models_dev_live.rs`.

### Env knobs (tests / dogfood / offline)

| Variable | Effect |
|---|---|
| `CODEWHALE_MODELS_DEV_URL` | Override base URL or full `*.json` catalog URL |
| `CODEWHALE_MODELS_DEV_PATH` | Load catalog from a local file; skip network |
| `CODEWHALE_DISABLE_MODELS_DEV_FETCH` | Truthy → never hit the network (`1` / `true` / `yes` / `on`) |

Defaults:

- Catalog URL: `https://models.dev/catalog.json`
- TTL: `24 * 60 * 60` seconds (`DEFAULT_MODELS_DEV_TTL_SECS`)
- Cache file name: `models-dev-catalog.json` under the CodeWhale `catalog`
  state dir

Freshness values exposed for UI / status chips: `bundled` | `live` | `stale` |
`failed`.

---

## What does **not** auto-update (repo / release)

These stay hand-maintained or release-lane work until a scheduled PR lands:

| Surface | Why it drifts |
|---|---|
| `models_dev.bundled.json` | Offline seed; intentionally smaller than full Models.dev |
| `model_catalog.bundled.json` | Compact TUI seed |
| `provider_defaults.rs` / default model IDs | Product choice, not pure catalog dump |
| Static tables in `models.rs` | Fallback heuristics when catalog misses a row |
| Hand-curated `pricing.rs` rows | Vendor billing quirks; not always in Models.dev |
| New `ProviderKind` / wire dialect | Needs code, not only JSON |

Runtime live refresh **does not** rewrite those files. Users on a recent
install with network still see new Models.dev rows; fresh clones offline, CI
hermetic runs, and first-boot without cache still depend on the seed.

---

## Maintainer tooling (no LLM)

### Validate / dry-run fetch

```bash
# Fetch Models.dev + print counts (never writes disk)
python3 scripts/catalog_models_dev.py refresh

# Validate the committed offline seed still parses as Models.dev-shaped JSON
python3 scripts/catalog_models_dev.py snapshot --check \
  crates/config/assets/models_dev.bundled.json

# OpenRouter public /models listing (no API key), dry-run only
python3 scripts/catalog_models_dev.py refresh --provider openrouter \
  --sort newest --limit 100
```

Design constraints of the script (intentional):

- Public endpoints only — no `Authorization` headers, no API keys.
- Credential-shaped keys are scrubbed if present in remote JSON.
- **Disk writes are disabled** (`--write` / `--write-cache` fail closed).
  Staging a new seed is a separate maintainer step so remote JSON is never
  blindly committed by automation without review.

### Staging a new offline seed (manual)

1. Fetch Models.dev to a local file (curl / browser), or use
   `CODEWHALE_MODELS_DEV_PATH` against a saved copy.
2. Scrub to the allowlisted shape (`models`, `providers`, optional `_meta`).
   Prefer the script’s public-document rules as the checklist.
3. Keep seed **compact** — verified defaults for shipped providers, not a
   full dump (see `_meta` on the existing asset).
4. `python3 scripts/catalog_models_dev.py snapshot --check <path>`.
5. Diff carefully: default wire IDs should stay aligned with
   `DEFAULT_*_MODEL` offline.
6. Open a normal PR. Do not force-push catalog history.

Optional: use a cheap model **on the PR** to summarize “new / removed /
default-risk” — never as the author of the JSON.

---

## Recommended scheduled job (not shipped yet)

Goal: keep the **in-repo offline seed** from rotting, without giving CI write
power over secrets or unsupervised LLM rewrites.

```text
cron (daily or weekly)
  → fetch Models.dev (public, no keys)
  → validate shape + scrub
  → compare against crates/config/assets/models_dev.bundled.json
     (and optionally report new ids vs provider defaults)
  → if material change: open PR
       title: chore(catalog): refresh Models.dev offline seed
  → optional: agent comments a human-readable diff summary on the PR
```

### In scope for automation

- Deterministic catalog ingest from Models.dev
- Secret-free PR diffs
- Drift reports (new model ids, missing defaults, pricing presence)

### Out of scope for automation

- Claude Pro/Max / subscription OAuth “model discovery” (not a supported
  third-party path; Anthropic expects API keys for third-party tools)
- LLM-authored edits to `models.rs` / `provider.rs` without review
- Force-pushing `main` or silent asset rewrites on the default branch
- Treating Models.dev as the only truth for OAuth-scoped routes (Codex
  roster remains special-cased)

### Suggested workflow home

`CodeWhale/.github/workflows/catalog-refresh.yml` (or similar), reusing
`scripts/catalog_models_dev.py` after a deliberate **write-safe** extension
that only runs in CI with a bot token for PR creation — still not on
`workflow_dispatch` without review if writes land in-repo.

Nightly today (`/.github/workflows/nightly.yml`) builds release artifacts
only; it does **not** refresh catalogs.

---

## Do we need a “model dedicated to updating models”?

**No for the core loop.**

| Job | Right tool |
|---|---|
| Keep known models/windows/prices from Models.dev fresh for users | Runtime live fetch (already shipped) |
| Keep offline seed + release assets current in git | Scheduled CI → PR (to build) |
| Decide whether to bump a product default model | Human (or agent *review* on the PR) |
| Wire a brand-new provider kind / dialect | Human PR + tests |

An LLM is optional **review** of a catalog PR. It is a poor **source of
truth** for catalog JSON.

---

## Auth note (Claude / Anthropic)

Anthropic model **catalog** refresh does not require Claude Pro/Max OAuth.
Models.dev is public. CodeWhale’s Anthropic route remains **API-key-based**
for inference (`ANTHROPIC_API_KEY`). Do not couple catalog automation to
subscription OAuth or Claude Code identity headers.

---

## Quick operator checklist

- [ ] Running install: confirm network not blocked; optional
      `/model refresh` after a big vendor launch.
- [ ] Offline / CI hermetic: set `CODEWHALE_DISABLE_MODELS_DEV_FETCH=1` or
      point `CODEWHALE_MODELS_DEV_PATH` at a fixture.
- [ ] Before release: `snapshot --check` on the bundled seed; skim
      `MODEL_PROVIDER_AUDIT.md` for known drift.
- [ ] After Models.dev adds a major family you ship by default: consider
      seed PR + default-model decision separately.
- [ ] Never paste API keys into catalog assets or the automation script env
      for Models.dev refresh.

---

## Issue / design anchors

- Live Models.dev layer: #4187
- Bundled seed demoted (not competing truth): #4188
- Catalog automation script (validate / dry-run): #4117
- Deeper metadata inventory and drift list: `docs/MODEL_PROVIDER_AUDIT.md`
