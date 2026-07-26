# Localization Matrix

Canonical tracking document for every locale Codewhale ships, is actively
building, is planning, or has explicitly deferred.

> **Scope note (2026-07-12):** this matrix covers three surfaces — the TUI
> locale packs (`crates/tui/locales/`), the translated READMEs (repo root),
> and the website (`web/`). The three ship on different cadences, so a
> locale can be **shipped** on one surface and **planned** on another; the
> per-surface tables below are the per-surface truth. The website registry
> is `web/lib/i18n/config.ts` (`ALL_LOCALES`): the locale switcher and route
> generation both derive from it.

Customer-visible copy also follows the [Codewhale voice and terminal
charter](VOICE.md); commands, key names, and glyphs remain code-owned around
localized prose.

Last updated: 2026-07-12.
Source-of-truth README: `README.md` (English, post-#3087).

## Status legend

| Status | Meaning |
|--------|---------|
| **shipped** | Live on codewhale.net and/or published as a standalone README |
| **partial** | Shipped but missing sections; actively being filled in |
| **planned** | Explicitly prioritized for the next wave |
| **deferred** | Acknowledged as wanted but not yet scheduled; needs layout QA, bridge support, or community champion |

---

## TUI locale packs

The TUI packs under `crates/tui/locales/` are the largest translation
surface in the repo. `en.json` is the reference; a pack is **complete**
only at exact raw key parity with it, enforced by
`scripts/check-tui-locale-parity.py` (CI) and the parity tests in
`crates/tui/src/localization.rs`. See `crates/tui/locales/AGENTS.md` for the
authoring contract.

| Locale | File | Keys vs `en.json` (1128) | Status | Notes |
|--------|------|--------------------------|--------|-------|
| English | `en.json` | 1128/1128 | **shipped** | Reference pack. |
| Japanese | `ja.json` | 1128/1128 | **shipped** | Complete. |
| Simplified Chinese | `zh-Hans.json` | 1128/1128 | **shipped** | Complete. |
| Traditional Chinese | `zh-Hant.json` | 478/1128 | **partial** | Setup core only; missing keys fall back to English at runtime. Deliberate scope per #4057. |
| Brazilian Portuguese | `pt-BR.json` | 1128/1128 | **shipped** | Complete. |
| Latin American Spanish | `es-419.json` | 1128/1128 | **shipped** | Complete. Note the website tracks `es` as deferred — the shipped TUI pack is Latin American Spanish, not `es-ES`. |
| Vietnamese | `vi.json` | 1128/1128 | **shipped** | Complete. |
| Korean | `ko.json` | 1128/1128 | **shipped** | Complete. |

## Website locales

| Locale | Code | Status | Notes |
|--------|------|--------|-------|
| English | `en` | **shipped** | Source text. Every page has an EN route. |
| Simplified Chinese | `zh` | **shipped** | Full parity with EN on all first-class pages. |
| Japanese | `ja` | **planned** | README exists (`README.ja-JP.md`); website route not yet live. Depends on locale-switcher supporting >2 languages and dictionary scaffolding (#3091). |
| Vietnamese | `vi` | **planned** | README exists (`README.vi.md`); same dependencies as Japanese (#3091). |
| Korean | `ko` | **planned** | README exists (`README.ko-KR.md`); #3093 next-wave locale. |
| Russian | `ru` | **planned** | **Next-priority locale.** No README yet; explicitly scoped for #3092. Latin+Cyrillic layout is established in the CSS font stack; needs dictionary + route scaffolding. |
| Spanish | `es` | **deferred** | #3093 next-wave. |
| Brazilian Portuguese | `pt-BR` | **deferred** | #3093 next-wave. |
| Arabic | `ar` | **deferred** | RTL candidate. Deferred until layout/typography QA exists (bidirectional text, mirrored chrome, number formatting). |

## README locales

| Locale | File | Status | Parity check |
|--------|------|--------|-------------|
| English | `README.md` | **shipped** | Canonical source |
| Simplified Chinese | `README.zh-CN.md` | **shipped** | Manual review per release |
| Japanese | `README.ja-JP.md` | **shipped** | Manual review per release |
| Vietnamese | `README.vi.md` | **shipped** | Manual review per release |
| Korean | `README.ko-KR.md` | **shipped** | Manual review per release |
| Russian | _(not yet created)_ | **planned** | #3092 |

## Drift checks

| Check | Tool | Status |
|-------|------|--------|
| TUI pack key parity with `en.json` (complete packs) | `scripts/check-tui-locale-parity.py` + parity tests in `crates/tui/src/localization.rs` | **Shipped** (CI Lint job) |
| README translations stay in sync with `README.md` | `scripts/check-readme-translations.py` | **Shipped** (CI Lint job) |
| README locale links symmetric | `scripts/check-readme-locales.sh` | **Shipped** (CI Lint job) |
| Website dictionaries cover all shipped locales | `npm run check:locales` (vitest) | Planned — tracked by #3091 |
| Accept-Language routes to all shipped locales | Middleware test | Planned — tracked by #3091 |
| Locale selector lists all shipped locales | Component test | Planned — tracked by #3091 |

## How to add a locale

A locale is not "added" until all three surfaces below either ship it or
carry an explicit `planned`/`partial`/`deferred` row in this matrix.

### 1. TUI pack

1. Create `crates/tui/locales/<tag>.json` with every key in `en.json`,
   following `crates/tui/locales/AGENTS.md` (placeholders stay literal;
   product terms stay English per pack convention; preserve intentional
   leading/trailing spaces).
2. Add the `Locale` variant plus its `tag`/`translation_target_name`/
   `parse_locale`/`shipped`/`shipped_complete` arms in
   `crates/tui/src/localization.rs`, and the `include_str!` arm in the
   test module.
3. Wire the pickers and displays that enumerate locales: onboarding
   language picker (`crates/tui/src/tui/onboarding/language.rs` — a test
   forces every shipped locale to be offered), setup-wizard match arms,
   and the locale display arms in the `/config` and changelog commands.
4. Run `python3 scripts/check-tui-locale-parity.py` and
   `cargo test -p codewhale-tui localization`.
5. If the pack must ship incomplete, declare it partial (see `zh-Hant` /
   #4057): keep it out of `shipped_complete()`, mark it in
   `is_partial_pack()`, and add it to `PARTIAL_PACKS` in
   `scripts/check-tui-locale-parity.py` with a tracking issue.

### 2. README

1. Translate `README.md` into `README.<tag>.md`, preserving structure,
   commands, and the #3087 factual history.
2. Cross-link it from the language line in `README.md` and from the other
   translated READMEs.
3. Restamp per `scripts/check-readme-translations.py`, then run
   `python3 scripts/check-readme-translations.py` and
   `bash scripts/check-readme-locales.sh`.

### 3. Website

1. Add the locale to `ALL_LOCALES` in `web/lib/i18n/config.ts` — the
   switcher and routes derive from it, so no per-locale switcher edit is
   needed. Use the `partial` status for locales that ship incomplete
   (e.g. a `zh-Hant`-style scoped pack).
2. Scaffold translation dictionaries under
   `web/lib/i18n/dictionaries/<code>/` (#3091 layer).
3. Verify `web/middleware.ts` routes the tag (base tags route with no
   middleware change).
4. Run `cd web && npm run check:locales && npm test && npm run build`.

### 4. Matrix

Update the TUI, README, and Website tables above — one row per surface,
with per-surface status.

## Assessments

### Galician (`gl`) and Basque (`eu`) — 2026-07-25, per #4749

Assessed alongside the Catalan pack (#4749 / #4788), which asked whether
Galician and Basque are "similar-value European additions" worth shipping
in the same wave.

**Decision: defer both.** Rationale:

- The case #4788 makes for Catalan is specifically that it "has an
  unusually strong software-localization tradition and an active volunteer
  community" — a review-capacity argument, not a market-size one. That
  argument does not transfer: Galician and Basque have materially smaller
  localization communities, so a pack for either would ship with no
  realistic path to native-speaker review.
- Galician speakers have a workable fallback already: the shipped
  `es-419` pack (and `pt-BR` is lexically close). Basque is a language
  isolate with no fallback proximity — its per-string review cost is the
  highest of the three, and machine-translated Basque is the least
  trustworthy of the three.
- There is no natural "ship together" grouping: the v0.9.2 wave already
  bundles the locales that share acceptance criteria (Latin-script
  fr/de/ca/id, Cyrillic uk, Devanagari hi). gl/eu share only the
  review-capacity constraint, which neither clears.

Revisit when a native-speaker champion appears for either language, or if
Catalan uptake after v0.9.2 suggests demand. Both base tags (`gl`, `eu`)
route through `web/middleware.ts` with no middleware change when that
happens.

## Related issues

- #3091 — Website parity with JA + VI README locales
- #3092 — Russian README + website localization
- #3093 — Korean, Spanish, Brazilian Portuguese next-wave locales
- #3087 — Post-rebrand README source text refresh
- #4057 — `zh-Hant` scoped as a partial TUI pack with English fallback
- #4787 — This matrix's TUI table + the locale-drift CI gates
- #4788 — French, German, Catalan TUI localization
- #4789 — Indonesian localization
- #4790 — Hindi localization + Devanagari terminal-shaping spike
- #4791 — Ukrainian localization alongside Russian
- #4749 — Catalan UI language + Galician/Basque assessment
